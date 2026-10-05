//! `SetData` - attach arbitrary data to shapes.
//!
//! This replaced `Fill` and `Outline`. Paint was four hardcoded vectors and
//! two instructions; adding "stroke dash" would have meant a fifth vector, a
//! third instruction and another `Emit` branch, and milling and 3-D print
//! want attributes that are not paint at all. One open map covers all of it
//! and `Emit` interprets what it recognises.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::*;

/// Merges a map of values into the data of every targeted shape.
///
/// Three levels of address, narrowing as you supply more:
///
/// * `get_group` alone — every shape in the group
/// * `+ get_index` — every shape in that slot
/// * `+ shape_index` — one shape within that slot
///
/// Merging rather than replacing means setting `fill` does not silently drop
/// a `stroke` set earlier. A `null` value removes a key; `replace: true`
/// clears the map first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetData {
    pub get_group: String,
    #[serde(default)]
    pub get_index: Option<Index>,
    /// Position *within the slot*, not a shape id - shape numbering is an
    /// internal detail that `Data::from` reorders.
    #[serde(default)]
    pub shape_index: Option<Index>,
    /// The values to merge. String values expand `{expression}`.
    pub data: Map<String, Value>,
    #[serde(default)]
    pub replace: bool,
}

impl Query for SetData {
    fn query(&mut self, data_instruction: &mut Data) -> Result<(), String> {
        let group = resolve_template(data_instruction, &self.get_group, "SetData.get_group")?;
        let get_index = match &self.get_index {
            Some(index) => Some(index.resolve(data_instruction, "SetData.get_index")?),
            None => None,
        };
        let shape_index = match &self.shape_index {
            Some(index) => Some(index.resolve(data_instruction, "SetData.shape_index")?),
            None => None,
        };

        // Expand templates before taking any lock - evaluating needs the JS
        // context, which lives on the same struct as the stores.
        let mut resolved = Map::new();
        for (key, value) in &self.data {
            let value = match value {
                Value::String(text) => Value::String(resolve_template(
                    data_instruction,
                    text,
                    &format!("SetData.data.{key}"),
                )?),
                other => other.clone(),
            };
            resolved.insert(key.clone(), value);
        }

        let targets: Vec<usize> = {
            let groups = data_instruction.groups.lock().unwrap();
            let entries = groups
                .get(&group)
                .ok_or_else(|| format!("SetData: unknown group '{group}'"))?;
            match (get_index, shape_index) {
                (Some(slot), Some(position)) => {
                    let indices = entries.get(slot).ok_or_else(|| {
                        format!(
                            "SetData: group '{group}' has no slot {slot} (it has {})",
                            entries.len()
                        )
                    })?;
                    let index = *indices.get(position).ok_or_else(|| {
                        format!(
                            "SetData: group '{group}' slot {slot} has no shape {position} \
                             (it has {})",
                            indices.len()
                        )
                    })?;
                    vec![index]
                }
                (Some(slot), None) => entries
                    .get(slot)
                    .ok_or_else(|| {
                        format!(
                            "SetData: group '{group}' has no slot {slot} (it has {})",
                            entries.len()
                        )
                    })?
                    .clone(),
                // Narrowing to a shape without saying which slot is
                // ambiguous, not a shorthand for "slot 0".
                (None, Some(_)) => {
                    return Err(
                        "SetData.shape_index needs get_index too - which slot is it in?".into()
                    )
                }
                (None, None) => entries.iter().flatten().copied().collect(),
            }
        };

        let mut store = data_instruction.shape_data.lock().unwrap();
        for index in targets {
            let Some(map) = store.get_mut(index) else {
                continue;
            };
            if self.replace {
                map.clear();
            }
            for (key, value) in &resolved {
                if value.is_null() {
                    map.remove(key);
                } else {
                    map.insert(key.clone(), value.clone());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::*;
    use serde_json::json;

    fn data_with_two_shapes() -> Data {
        use geo::{LineString, Polygon};
        let square = |x: f64| {
            Polygon::new(
                LineString::from(vec![(x, 0.0), (x + 1.0, 0.0), (x + 1.0, 1.0), (x, 1.0)]),
                vec![],
            )
        };
        let data = Data::from(vec![square(0.0), square(10.0)]);
        data.sync_style_lengths();
        {
            let mut groups = data.groups.lock().unwrap();
            groups.insert("parts".into(), vec![vec![0, 1]]);
        }
        data
    }

    fn set(data: &mut Data, map: serde_json::Value, shape_index: Option<Index>, replace: bool) {
        SetData {
            get_group: "parts".into(),
            get_index: Some(Index::Literal(0)),
            shape_index,
            data: map.as_object().unwrap().clone(),
            replace,
        }
        .query(data)
        .expect("SetData should succeed");
    }

    #[test]
    fn merges_rather_than_replacing() {
        let mut data = data_with_two_shapes();
        set(&mut data, json!({"fill": "red"}), None, false);
        set(&mut data, json!({"stroke": "black"}), None, false);

        let store = data.shape_data.lock().unwrap();
        // Setting stroke must not have dropped the fill.
        assert_eq!(store[0]["fill"], json!("red"));
        assert_eq!(store[0]["stroke"], json!("black"));
    }

    #[test]
    fn null_removes_and_replace_clears() {
        let mut data = data_with_two_shapes();
        set(&mut data, json!({"fill": "red", "tool": 4}), None, false);
        set(&mut data, json!({"fill": null}), None, false);
        {
            let store = data.shape_data.lock().unwrap();
            assert!(!store[0].contains_key("fill"));
            assert_eq!(store[0]["tool"], json!(4));
        }

        set(&mut data, json!({"fill": "blue"}), None, true);
        let store = data.shape_data.lock().unwrap();
        assert!(!store[0].contains_key("tool"), "replace should have cleared it");
        assert_eq!(store[0]["fill"], json!("blue"));
    }

    /// The four-argument form: one shape inside a slot, not the whole slot.
    #[test]
    fn shape_index_targets_a_single_shape() {
        let mut data = data_with_two_shapes();
        set(&mut data, json!({"fill": "red"}), Some(Index::Literal(1)), false);

        let store = data.shape_data.lock().unwrap();
        assert!(!store[0].contains_key("fill"), "shape 0 should be untouched");
        assert_eq!(store[1]["fill"], json!("red"));
    }

    #[test]
    fn values_expand_expressions() {
        let mut data = data_with_two_shapes();
        RunCode { code: "n = 7;".into() }.query(&mut data).unwrap();
        set(&mut data, json!({"layer": "sheet-{n}"}), None, false);

        let store = data.shape_data.lock().unwrap();
        assert_eq!(store[0]["layer"], json!("sheet-7"));
    }

    #[test]
    fn a_shape_index_without_a_slot_is_an_error() {
        let mut data = data_with_two_shapes();
        let err = SetData {
            get_group: "parts".into(),
            get_index: None,
            shape_index: Some(Index::Literal(0)),
            data: serde_json::Map::new(),
            replace: false,
        }
        .query(&mut data)
        .expect_err("ambiguous address must not silently pick slot 0");
        assert!(err.contains("needs get_index"), "got: {err}");
    }
}
