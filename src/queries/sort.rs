use boa_engine::{JsValue, Source, js_string, property::Attribute};
use serde::{Deserialize, Serialize};

use crate::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sort {
    pub set_group: String,
    pub get_group: String,
    pub compare: String,
}

impl Query for Sort {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let shapes_indexes = {
            let groups = data.groups.lock().unwrap();
            let Some(shapes_indexes) = groups.get(&self.get_group) else {
                return Err(format!("Could not find '{}' in groups.", self.get_group));
            };
            shapes_indexes.clone()
        };

        let mut indexes: Vec<usize> = (0..shapes_indexes.len()).collect();
        indexes.sort_by(|l, r| {
            data.context
                .register_global_property(js_string!("l"), *l, Attribute::all())
                .expect("property shouldn't exist");

            data.context
                .register_global_property(js_string!("r"), *r, Attribute::all())
                .expect("property shouldn't exist");

            match data.context.eval(Source::from_bytes(&self.compare)) {
                Ok(JsValue::Boolean(value)) => {
                    if value {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Greater
                    }
                }
                _ => std::cmp::Ordering::Equal,
            }
        });

        let mut new_group = Vec::new();

        for index in indexes {
            new_group.push(shapes_indexes[index].clone());
        }

        let mut groups = data.groups.lock().unwrap();
        groups.insert(self.set_group.clone(), new_group);

        Ok(())
    }
}
