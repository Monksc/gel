use boa_engine::{JsValue, Source, js_string, property::Attribute};

use crate::*;

#[derive(Debug, Clone)]
pub struct Filter {
    pub set_group: String,
    pub get_group: String,
    pub code: String,
}

impl Query for Filter {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let shapes_indexes = {
            let groups = data.groups.lock().unwrap();
            let Some(shapes_indexes) = groups.get(&self.get_group) else {
                return Err(format!("Could not find '{}' in groups.", self.get_group));
            };
            shapes_indexes.clone()
        };

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

        let mut groups = data.groups.lock().unwrap();
        groups.insert(self.set_group.clone(), new_group);

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
