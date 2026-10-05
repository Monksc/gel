use boa_engine::{JsValue, Source, js_string, property::Attribute};
use serde::{Deserialize, Serialize};

use crate::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Filter {
    pub set_group: String,
    pub get_group: String,
    pub code: String,
}

impl Query for Filter {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let get_group = resolve_template(data, &self.get_group, "Filter.get_group")?;
        let set_group = resolve_template(data, &self.set_group, "Filter.set_group")?;

        let shapes_indexes = {
            let groups = data.groups.lock().unwrap();
            let Some(shapes_indexes) = groups.get(&get_group) else {
                return Err(format!("Could not find '{}' in groups.", get_group));
            };
            shapes_indexes.clone()
        };

        // `i` is a SLOT ordinal. Marking the group being iterated is what
        // lets `area(i)` / `get_data(i, k)` resolve that ordinal to the right
        // shapes - without it a bare integer means a shape index, which is
        // only the same thing when slots are `[[0],[1],[2]...]`.
        let current = data.current_group.clone();
        let previous = current.lock().unwrap().replace(get_group.clone());

        let new_group = shapes_indexes
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                data.context
                    .register_global_property(js_string!("i"), *index, Attribute::all())
                    .expect("property shouldn't exist");

                match data.context.eval(Source::from_bytes(&self.code)) {
                    Ok(JsValue::Boolean(value)) => value,
                    _ => false,
                }
            })
            .map(|(_, shapes_indexes)| shapes_indexes.clone())
            .collect::<Vec<Vec<usize>>>();

        *current.lock().unwrap() = previous;

        let mut groups = data.groups.lock().unwrap();
        groups.insert(set_group.clone(), new_group);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use geo::polygon;

    use crate::*;

    #[test]
    fn it_works() {
        let mut data = Data::from(vec![polygon! {(0.0, 0.0).into()}]);

        let mut groupby = Filter {
            set_group: "output".into(),
            get_group: "main".into(),
            code: "true".into(),
        };

        if let Err(err) = groupby.query(&mut data) {
            eprintln!("Error: {}", err);
            assert!(false);
        }

        let groups = data.groups.lock().unwrap();

        let main = groups.get("main");
        assert!(main.is_some());
        let main = main.unwrap();
        assert_eq!(main, &vec![vec![0]]);

        let output = groups.get("output");
        assert!(output.is_some());
        let output = output.unwrap();
        assert_eq!(output, &vec![vec![0]]);
    }

    /// Regression: `i` is a slot ordinal, so a bare `area(i)` has to resolve
    /// against the group being iterated.
    ///
    /// A group derived by an earlier `Filter` has non-identity shape indices
    /// (`[[3],[5],[8]]`, not `[[0],[1],[2]]`), so reading `i` as a shape
    /// index measured an unrelated shape - silently, with no error. On real
    /// artwork this scored 13 where the explicit form scored 39.
    #[test]
    fn a_bare_index_follows_the_group_being_iterated() {
        use geo::{LineString, Polygon};
        let square = |size: f64, offset: f64| {
            Polygon::new(
                LineString::from(vec![
                    (offset, 0.0),
                    (offset + size, 0.0),
                    (offset + size, size),
                    (offset, size),
                ]),
                vec![],
            )
        };
        // Areas 1, 4, 9, 16, spread out so none contains another.
        let mut data = Data::from(vec![
            square(1.0, 0.0),
            square(2.0, 100.0),
            square(3.0, 200.0),
            square(4.0, 300.0),
        ]);

        let mut run = |get: &str, set: &str, code: &str| {
            Filter {
                get_group: get.into(),
                set_group: set.into(),
                code: code.into(),
            }
            .query(&mut data)
            .expect("filter should run");
        };

        // `big` gets slots pointing at whichever shapes have area > 2 - not
        // shapes 0..n, which is the whole point.
        run("main", "big", "area(i) > 2.0");
        run("big", "bare", "area(i) > 8.0");
        run("big", "explicit", "area('big', i) > 8.0");
        // `center` reaches the shapes through the shared `get_polygons`
        // helper rather than its own match arm - a second code path that had
        // the same bug.
        run("big", "c_bare", "center(i).x > 150.0");
        run("big", "c_explicit", "center('big', i).x > 150.0");

        let groups = data.groups.lock().unwrap();
        assert_eq!(groups["big"].len(), 3, "areas 4, 9 and 16 are over 2");
        assert_eq!(
            groups["bare"], groups["explicit"],
            "a bare index must agree with the explicit group form"
        );
        assert_eq!(groups["bare"].len(), 2, "areas 9 and 16 are over 8");
        assert_eq!(
            groups["c_bare"], groups["c_explicit"],
            "get_polygons must follow the iterated group too"
        );
        assert_eq!(groups["c_bare"].len(), 2, "the squares at x=200 and x=300");
    }
}
