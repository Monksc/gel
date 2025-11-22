use boa_engine::Source;
use geo::{AffineOps, AffineTransform};
use serde::{Deserialize, Serialize};

use crate::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transformation {
    pub set_group: String,
    pub get_group: String,
    pub transformation: [String; 6],
}

impl Query for Transformation {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let shapes_indexes = {
            let groups = data.groups.lock().unwrap();
            let Some(shapes_indexes) = groups.get(&self.get_group) else {
                return Err(format!("Could not find '{}' in groups.", self.get_group));
            };
            shapes_indexes.clone()
        };

        let mut t_matrix = [0.; 6];
        for i in 0..6 {
            if let Ok(value) = data
                .context
                .eval(Source::from_bytes(&self.transformation[i]))
            {
                if let Ok(value) = value.to_f32(&mut data.context) {
                    t_matrix[i] = value as f64;
                }
            }
        }

        let transformation = AffineTransform::new(
            t_matrix[0],
            t_matrix[1],
            t_matrix[2],
            t_matrix[3],
            t_matrix[4],
            t_matrix[5],
        );

        let mut new_group = Vec::new();

        let mut shapes = data.shapes.lock().unwrap();

        let mut new_shapes = Vec::new();

        for shapes_index in shapes_indexes {
            let mut ng = Vec::new();
            for index in shapes_index {
                ng.push(new_shapes.len() + shapes.len());
                new_shapes.push(shapes[index].affine_transform(&transformation));
            }
            new_group.push(ng);
        }

        let mut groups = data.groups.lock().unwrap();
        groups.insert(self.set_group.clone(), new_group);

        shapes.append(&mut new_shapes);

        Ok(())
    }
}
