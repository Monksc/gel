//! The geometry instructions: `Offset`, `Union`, `Intersect`, `Difference`,
//! `Flatten`, `Copy`.
//!
//! These operations already existed as JS globals, but a global can only be
//! reached by evaluating an expression - so the only way to call one was to
//! run a `Filter` over a deliberately one-element group purely for the side
//! effect, and bolt a `> 0` on the end so the predicate returned a boolean.
//! That is what the layout pipeline's `run_once` helper does. As instructions
//! they are statements, which is what they always were.
//!
//! Both paths call the same `*_slot` functions in `data.rs`, so the JS form
//! and the instruction form cannot drift apart.
//!
//! Numeric fields are expression strings, matching `Transformation`'s matrix
//! cells and `Kerning`'s `epsilon`/`space`. That lets an amount refer to
//! anything a `RunCode` defined - `"4 * thou"` rather than a baked `0.004`.

use boa_engine::{JsValue, Source};
use geo::MultiPolygon;
use geo_clipper::Clipper;
use serde::{Deserialize, Serialize};

use crate::*;

/// Evaluates an expression against the shared JS context and returns a number.
fn eval_number(data: &mut Data, expression: &str, field: &str) -> Result<f64, String> {
    let value = data
        .context
        .eval(Source::from_bytes(expression))
        .map_err(|err| format!("could not evaluate {field} ({expression:?}): {err}"))?;

    match value {
        JsValue::Integer(n) => Ok(n as f64),
        JsValue::Rational(n) => Ok(n),
        other => Err(format!(
            "{field} ({expression:?}) evaluated to {other:?}, which is not a number"
        )),
    }
}

/// Grows (or, with a negative amount, shrinks) the shapes in a slot.
///
/// A negative amount is an inset, which is not an edge case - the Laser
/// guide calls for one explicitly on surface-painted slider parts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Offset {
    pub get_group: String,
    #[serde(default)]
    pub get_index: Index,
    pub set_group: String,
    #[serde(default)]
    pub set_index: Index,
    /// Expression, in drawing units.
    pub amount: String,
    /// `"round"` (default), `"miter"` or `"square"`.
    #[serde(default)]
    pub join: Option<String>,
    /// Miter limit / round precision. Expression; defaults to 2.
    #[serde(default)]
    pub join_value: Option<String>,
}

impl Query for Offset {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let amount = eval_number(data, &self.amount, "Offset.amount")?;
        let join = self.join.clone().unwrap_or_else(|| "round".into());
        let join_value = match &self.join_value {
            Some(expression) => eval_number(data, expression, "Offset.join_value")?,
            None => 2.0,
        };

        let get_group = resolve_template(data, &self.get_group, "Offset.get_group")?;
        let set_group = resolve_template(data, &self.set_group, "Offset.set_group")?;
        let get_index = self.get_index.resolve(data, "Offset.get_index")?;
        let set_index = self.set_index.resolve(data, "Offset.set_index")?;

        offset_slot(
            &data.shapes,
            &data.depths,
            &data.groups,
            &get_group,
            get_index,
            &set_group,
            set_index,
            amount,
            &join,
            join_value,
        );
        Ok(())
    }
}

/// Operands for a two-input boolean. `get_*` is the left side, `with_*` the
/// right - which matters for `Difference`, where the result is left minus
/// right.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Boolean {
    pub get_group: String,
    #[serde(default)]
    pub get_index: Index,
    pub with_group: String,
    #[serde(default)]
    pub with_index: Index,
    pub set_group: String,
    #[serde(default)]
    pub set_index: Index,
}

impl Boolean {
    fn run(
        &self,
        data: &mut Data,
        op: fn(&MultiPolygon, &MultiPolygon, f64) -> MultiPolygon,
    ) -> Result<(), String> {
        let get_group = resolve_template(data, &self.get_group, "get_group")?;
        let with_group = resolve_template(data, &self.with_group, "with_group")?;
        let set_group = resolve_template(data, &self.set_group, "set_group")?;
        let get_index = self.get_index.resolve(data, "get_index")?;
        let with_index = self.with_index.resolve(data, "with_index")?;
        let set_index = self.set_index.resolve(data, "set_index")?;

        boolean_slot(
            &data.shapes,
            &data.depths,
            &data.groups,
            &get_group,
            get_index,
            &with_group,
            with_index,
            &set_group,
            set_index,
            op,
        );
        Ok(())
    }
}

/// Merges overlapping geometry into one set.
///
/// Note this is *geometric* union, not set union on group membership: two
/// overlapping rectangles become one polygon. Simply sharing a slot already
/// gives the set-union behaviour for measurement and addressing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Union(pub Boolean);

impl Query for Union {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        self.0.run(data, |a, b, f| Clipper::union(a, b, f))
    }
}

/// Keeps only the geometry common to both operands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intersect(pub Boolean);

impl Query for Intersect {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        self.0.run(data, |a, b, f| Clipper::intersection(a, b, f))
    }
}

/// `get` minus `with`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Difference(pub Boolean);

impl Query for Difference {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        self.0.run(data, |a, b, f| Clipper::difference(a, b, f))
    }
}

/// Collapses every slot of a group into one slot.
///
/// Offsets and booleans each resolve exactly one slot per operand, so anything
/// that needs a whole matched set as a single operand has to flatten first.
/// This only re-points membership; it creates no geometry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flatten {
    pub get_group: String,
    pub set_group: String,
    #[serde(default)]
    pub set_index: Index,
}

impl Query for Flatten {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let get_group = resolve_template(data, &self.get_group, "Flatten.get_group")?;
        let set_group = resolve_template(data, &self.set_group, "Flatten.set_group")?;
        let set_index = self.set_index.resolve(data, "Flatten.set_index")?;

        flatten_slot(&data.groups, &get_group, &set_group, set_index);
        Ok(())
    }
}

/// Duplicates shapes, rather than pointing a second name at the same ones.
///
/// Needed wherever an original has to survive a modification - the inlaid
/// procedure keeps unaltered text while the original gets offset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Copy {
    pub get_group: String,
    #[serde(default)]
    pub get_index: Index,
    pub set_group: String,
    #[serde(default)]
    pub set_index: Index,
}

impl Query for Copy {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let get_group = resolve_template(data, &self.get_group, "Copy.get_group")?;
        let set_group = resolve_template(data, &self.set_group, "Copy.set_group")?;
        let get_index = self.get_index.resolve(data, "Copy.get_index")?;
        let set_index = self.set_index.resolve(data, "Copy.set_index")?;

        copy_slot(
            &data.shapes,
            &data.depths,
            &data.groups,
            &get_group,
            get_index,
            &set_group,
            set_index,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use geo::polygon;

    use crate::*;

    fn unit_square() -> geo::Polygon {
        polygon![
            (x: 0.0, y: 0.0), (x: 1.0, y: 0.0), (x: 1.0, y: 1.0), (x: 0.0, y: 1.0)
        ]
    }

    /// geo_clipper works in integers scaled by `CLIPPER_FACTOR` (2^15), so
    /// results land within ~1/32768 (0.00003in) of the exact value - three
    /// orders of magnitude finer than the 0.004in offsets these ops are used
    /// for, but not exact.
    const CLIPPER_EPSILON: f64 = 1e-4;

    fn frame_of(data: &Data, group: &str) -> (f64, f64, f64, f64) {
        use geo::BoundingRect;
        let groups = data.groups.lock().unwrap();
        let shapes = data.shapes.lock().unwrap();
        let indices = &groups[group][0];
        let multi = geo::MultiPolygon::new(indices.iter().map(|&i| shapes[i].clone()).collect());
        let rect = multi.bounding_rect().expect("should have a bounding rect");
        (rect.min().x, rect.min().y, rect.max().x, rect.max().y)
    }

    #[test]
    fn offset_grows_by_the_amount_on_every_side() {
        let mut data = Data::from(vec![unit_square()]);

        data.query(vec![
            Instruction::RunCode(RunCode {
                code: "thou = 0.001;".into(),
            }),
            Instruction::Flatten(Flatten {
                get_group: "main".into(),
                set_group: "flat".into(),
                set_index: Index::Literal(0),
            }),
            // Expression, not a literal - proves amounts can reference RunCode.
            Instruction::Offset(Offset {
                get_group: "flat".into(),
                get_index: Index::Literal(0),
                set_group: "grown".into(),
                set_index: Index::Literal(0),
                amount: "100 * thou".into(),
                join: Some("miter".into()),
                join_value: None,
            }),
        ])
        .expect("program should run");

        let (min_x, min_y, max_x, max_y) = frame_of(&data, "grown");
        // 0.1 out on each side, so each axis grows by 0.2.
        assert!((min_x - -0.1).abs() < CLIPPER_EPSILON, "min_x was {min_x}");
        assert!((min_y - -0.1).abs() < CLIPPER_EPSILON, "min_y was {min_y}");
        assert!((max_x - 1.1).abs() < CLIPPER_EPSILON, "max_x was {max_x}");
        assert!((max_y - 1.1).abs() < CLIPPER_EPSILON, "max_y was {max_y}");
    }

    #[test]
    fn negative_offset_insets() {
        let mut data = Data::from(vec![unit_square()]);
        data.query(vec![
            Instruction::Flatten(Flatten {
                get_group: "main".into(),
                set_group: "flat".into(),
                set_index: Index::Literal(0),
            }),
            Instruction::Offset(Offset {
                get_group: "flat".into(),
                get_index: Index::Literal(0),
                set_group: "shrunk".into(),
                set_index: Index::Literal(0),
                amount: "-0.25".into(),
                join: Some("miter".into()),
                join_value: None,
            }),
        ])
        .expect("program should run");

        let (min_x, _, max_x, _) = frame_of(&data, "shrunk");
        assert!((min_x - 0.25).abs() < CLIPPER_EPSILON, "min_x was {min_x}");
        assert!((max_x - 0.75).abs() < CLIPPER_EPSILON, "max_x was {max_x}");
    }

    #[test]
    fn difference_removes_the_overlap() {
        // Two unit squares overlapping by half.
        let a = unit_square();
        let b = polygon![
            (x: 0.5, y: 0.0), (x: 1.5, y: 0.0), (x: 1.5, y: 1.0), (x: 0.5, y: 1.0)
        ];
        let mut data = Data::from(vec![a, b]);

        data.query(vec![
            // Selected by position, NOT by index: `Data::from` reorders
            // shapes (that's why `from_respect_indexes` hands back a mapping),
            // so `i == 0` is not the first polygon passed in.
            Instruction::Filter(Filter {
                set_group: "a".into(),
                get_group: "main".into(),
                code: "frame(i).min_x < 0.25".into(),
            }),
            Instruction::Filter(Filter {
                set_group: "b".into(),
                get_group: "main".into(),
                code: "frame(i).min_x >= 0.25".into(),
            }),
            Instruction::Flatten(Flatten {
                get_group: "a".into(),
                set_group: "a_flat".into(),
                set_index: Index::Literal(0),
            }),
            Instruction::Flatten(Flatten {
                get_group: "b".into(),
                set_group: "b_flat".into(),
                set_index: Index::Literal(0),
            }),
            Instruction::Difference(Difference(Boolean {
                get_group: "a_flat".into(),
                get_index: Index::Literal(0),
                with_group: "b_flat".into(),
                with_index: Index::Literal(0),
                set_group: "only_a".into(),
                set_index: Index::Literal(0),
            })),
        ])
        .expect("program should run");

        // a minus b is the left half of a.
        let (min_x, _, max_x, _) = frame_of(&data, "only_a");
        assert!((min_x - 0.0).abs() < CLIPPER_EPSILON, "min_x was {min_x}");
        assert!((max_x - 0.5).abs() < CLIPPER_EPSILON, "max_x was {max_x}");
    }

    #[test]
    fn copy_makes_new_shapes_not_an_alias() {
        let mut data = Data::from(vec![unit_square()]);
        let before = data.shapes.lock().unwrap().len();

        data.query(vec![
            Instruction::Flatten(Flatten {
                get_group: "main".into(),
                set_group: "flat".into(),
                set_index: Index::Literal(0),
            }),
            Instruction::Copy(Copy {
                get_group: "flat".into(),
                get_index: Index::Literal(0),
                set_group: "backup".into(),
                set_index: Index::Literal(0),
            }),
        ])
        .expect("program should run");

        let after = data.shapes.lock().unwrap().len();
        assert_eq!(after, before + 1, "copy should append a real shape");

        let groups = data.groups.lock().unwrap();
        assert_ne!(
            groups["backup"][0], groups["flat"][0],
            "copy must not point at the original indices"
        );
    }

    #[test]
    fn a_bad_amount_expression_is_an_error() {
        let mut data = Data::from(vec![unit_square()]);
        let result = data.query(vec![Instruction::Offset(Offset {
            get_group: "main".into(),
            get_index: Index::Literal(0),
            set_group: "out".into(),
            set_index: Index::Literal(0),
            amount: "no_such_value * 2".into(),
            join: None,
            join_value: None,
        })]);
        assert!(result.is_err(), "undefined value should fail, not silently offset by 0");
    }
}
