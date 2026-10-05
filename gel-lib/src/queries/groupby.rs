use boa_engine::{JsValue, Source, js_string, property::Attribute};
use serde::{Deserialize, Serialize};

use crate::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupBy {
    pub set_group: String,
    pub get_group: String,
    pub code: String,
}

impl Query for GroupBy {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let get_group = resolve_template(data, &self.get_group, "GroupBy.get_group")?;
        let set_group = resolve_template(data, &self.set_group, "GroupBy.set_group")?;

        let shapes_indexes = {
            let mut groups = data.groups.lock().unwrap();
            let Some(shapes_indexes) = groups.get(&get_group) else {
                return Err(format!("Could not find '{}' in groups.", get_group));
            };

            if shapes_indexes.is_empty() {
                groups.insert(set_group.clone(), Vec::new());
                return Ok(());
            }

            shapes_indexes.clone()
        };

        let mut new_groups = Vec::new();
        new_groups.push(shapes_indexes[0].clone());

        {
            let mut groups = data.groups.lock().unwrap();
            groups.insert(set_group.clone(), new_groups.clone());
        }

        // `i` is a slot ordinal of get_group, so mark it. NOTE `j` indexes
        // the accumulating set_group instead, and only one group can be
        // current - inside a GroupBy predicate, address `j` explicitly as
        // `area(set_group_name, j)`.
        let current = data.current_group.clone();
        let previous = current.lock().unwrap().replace(get_group.clone());

        'outer: for i in 1..shapes_indexes.len() {
            data.context
                .register_global_property(js_string!("i"), i, Attribute::all())
                .expect("property shouldn't exist");

            for j in 0..new_groups.len() {
                data.context
                    .register_global_property(js_string!("j"), j, Attribute::all())
                    .expect("property shouldn't exist");
                if let Ok(JsValue::Boolean(value)) =
                    data.context.eval(Source::from_bytes(&self.code))
                {
                    if value {
                        new_groups[j].append(&mut shapes_indexes[i].clone());

                        let mut groups = data.groups.lock().unwrap();
                        groups.insert(set_group.clone(), new_groups.clone());
                        continue 'outer;
                    }
                }
            }
            new_groups.push(shapes_indexes[i].clone());
            {
                let mut groups = data.groups.lock().unwrap();
                groups.insert(set_group.clone(), new_groups.clone());
            }
        }

        *current.lock().unwrap() = previous;

        {
            let mut groups = data.groups.lock().unwrap();
            groups.insert(set_group.clone(), new_groups);
        }

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

        let mut groupby = GroupBy {
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
}
