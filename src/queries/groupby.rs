use boa_engine::{JsValue, Source, js_string, property::Attribute};

use crate::*;

#[derive(Debug, Clone)]
pub struct GroupBy {
    pub set_group: String,
    pub get_group: String,
    pub code: String,
}

impl Query for GroupBy {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let shapes_indexes = {
            let mut groups = data.groups.lock().unwrap();
            let Some(shapes_indexes) = groups.get(&self.get_group) else {
                return Err(format!("Could not find '{}' in groups.", self.get_group));
            };

            if shapes_indexes.is_empty() {
                groups.insert(self.set_group.clone(), Vec::new());
                return Ok(());
            }

            shapes_indexes.clone()
        };

        let mut new_groups = Vec::new();
        new_groups.push(shapes_indexes[0].clone());

        {
            let mut groups = data.groups.lock().unwrap();
            groups.insert(self.set_group.clone(), new_groups.clone());
        }

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
                        groups.insert(self.set_group.clone(), new_groups.clone());
                        continue 'outer;
                    }
                }
            }
            new_groups.push(shapes_indexes[i].clone());
            {
                let mut groups = data.groups.lock().unwrap();
                groups.insert(self.set_group.clone(), new_groups.clone());
            }
        }
        {
            let mut groups = data.groups.lock().unwrap();
            groups.insert(self.set_group.clone(), new_groups);
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
            println!("Error: {}", err);
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
