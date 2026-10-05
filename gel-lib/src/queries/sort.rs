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
        let get_group = resolve_template(data, &self.get_group, "Sort.get_group")?;
        let set_group = resolve_template(data, &self.set_group, "Sort.set_group")?;

        let shapes_indexes = {
            let groups = data.groups.lock().unwrap();
            let Some(shapes_indexes) = groups.get(&get_group) else {
                return Err(format!("Could not find '{}' in groups.", get_group));
            };
            shapes_indexes.clone()
        };

        // `l` and `r` are SLOT ordinals of get_group - see Filter for why
        // the iterated group has to be marked.
        let current = data.current_group.clone();
        let previous = current.lock().unwrap().replace(get_group.clone());

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

        *current.lock().unwrap() = previous;

        let mut new_group = Vec::new();

        for index in indexes {
            new_group.push(shapes_indexes[index].clone());
        }

        let mut groups = data.groups.lock().unwrap();
        groups.insert(set_group.clone(), new_group);

        Ok(())
    }
}
