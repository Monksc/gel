use boa_engine::{JsValue, Source, js_string, property::Attribute};
use geo::{
    BoundingRect, Centroid, Contains, Distance, Euclidean, MultiPolygon, Polygon, Translate,
};

use crate::*;

#[derive(Debug, Clone)]
pub struct Kerning {
    pub set_group: String,
    pub get_group: String,
    pub set_inner_shapes: String,
    pub get_inner_shapes: String,
    pub borders_group: String,
    pub epsilon: String,
    pub space: String,
    pub respect_space: String,
}

#[derive(Debug)]
enum Direction {
    Left,
    Top,
    Right,
    Bottom,
    Center,
}

impl Query for Kerning {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let mut space: f64 = 0.0;
        if let Ok(value) = data.context.eval(Source::from_bytes(&self.space)) {
            if let Ok(value) = value.to_f32(&mut data.context) {
                space = value as f64;
            }
        }

        let mut epsilon: f64 = 0.0;
        if let Ok(value) = data.context.eval(Source::from_bytes(&self.epsilon)) {
            if let Ok(value) = value.to_f32(&mut data.context) {
                epsilon = value as f64;
            }
        }

        let (kerned_group, borders_group, mut inner_shapes) = {
            let groups = data.groups.lock().unwrap();
            let Some(shapes_indexes) = groups.get(&self.get_group) else {
                return Err(format!("Could not find '{}' in groups.", self.get_group));
            };
            let Some(borders_indexes) = groups.get(&self.borders_group) else {
                return Err(format!(
                    "Could not find '{}' in groups.",
                    self.borders_group
                ));
            };
            let Some(inner_shapes) = groups.get(&self.get_inner_shapes) else {
                return Err(format!(
                    "Could not find '{}' in groups.",
                    self.get_inner_shapes
                ));
            };
            (
                shapes_indexes.clone(),
                borders_indexes.clone(),
                inner_shapes
                    .into_iter()
                    .flatten()
                    .copied()
                    .collect::<std::collections::HashSet<usize>>(),
            )
        };

        let shapes = { data.shapes.lock().unwrap().clone() };

        let mut new_group = Vec::new();

        let mut new_inner_shapes = Vec::new();

        for (kerned_group_index, kerened_group) in kerned_group.iter().enumerate() {
            data.context
                .register_global_property(js_string!("i"), kerned_group_index, Attribute::all())
                .expect("property shouldn't exist");

            if kerened_group.len() < 2 {
                continue;
            }

            let Some(first) = kerened_group.first() else {
                continue;
            };

            let Some(center) = shapes[*first].centroid() else {
                continue;
            };

            // Calculate min / max x y

            let (mut min_x, mut min_y) = center.0.x_y();
            let (mut max_x, mut max_y) = center.0.x_y();

            let mut shapes_to_kern = vec![shapes[*first].clone()];

            // Finding out if its horizontal or verital kerning
            for index in 1..kerened_group.len() {
                let index = kerened_group[index];

                let Some(center) = shapes[index].centroid() else {
                    continue;
                };

                shapes_to_kern.push(shapes[index].clone());

                let (x, y) = center.0.x_y();

                if x < min_x {
                    min_x = x;
                }
                if x > max_x {
                    max_x = x;
                }

                if y < min_y {
                    min_y = y;
                }
                if y > max_y {
                    max_y = y;
                }
            }

            let is_horizontal = (max_x - min_x) >= (max_y - min_y);

            println!("Is Horizontal: {}", is_horizontal);

            // Sort the shapes by the way its kerning
            if is_horizontal {
                shapes_to_kern.sort_by(|l, r| {
                    l.bounding_rect()
                        .unwrap()
                        .min()
                        .x
                        .partial_cmp(&r.bounding_rect().unwrap().min().x)
                        .unwrap()
                });
            } else {
                shapes_to_kern.sort_by(|l, r| {
                    l.bounding_rect()
                        .unwrap()
                        .min()
                        .y
                        .partial_cmp(&r.bounding_rect().unwrap().min().y)
                        .unwrap()
                });
            }

            // Get kerning rules. Left, Right, Center adjustified

            let Some(shapes_to_kern_border) =
                MultiPolygon::new(shapes_to_kern.clone()).bounding_rect()
            else {
                continue;
            };

            let mut direction = None;
            for border in &borders_group {
                let multi_polygon = MultiPolygon::new(
                    border
                        .into_iter()
                        .map(|index| shapes[*index].clone())
                        .collect::<Vec<Polygon>>(),
                );

                let Some(rect) = multi_polygon.bounding_rect() else {
                    continue;
                };

                if rect.contains(&shapes_to_kern_border) {
                    if is_horizontal {
                        let l = shapes_to_kern_border.min().x - rect.min().x;
                        let r = rect.max().x - shapes_to_kern_border.max().x;

                        if (l - r).abs() < 0.1 {
                            direction = Some(Direction::Center);
                        } else if l > 2.0 * r {
                            direction = Some(Direction::Left);
                        } else if r > 1.1 * l
                            || l - 0.5 < shapes_to_kern[0].bounding_rect().unwrap().height()
                        {
                            direction = Some(Direction::Right);
                        } else {
                            direction = Some(Direction::Center);
                        }
                        break;
                    } else {
                        let b = shapes_to_kern_border.min().y - rect.min().y;
                        let u = rect.max().y - shapes_to_kern_border.max().y;

                        if b > 2.0 * u {
                            direction = Some(Direction::Bottom);
                        } else if u > 1.1 * b
                            || b < shapes_to_kern[0].bounding_rect().unwrap().width()
                        {
                            direction = Some(Direction::Top);
                        } else {
                            direction = Some(Direction::Center);
                        }
                        break;
                    }
                }
            }

            let Some(direction) = direction else {
                continue;
            };

            println!("Direction: {:?}", direction);

            // Push up or right but then rejustify

            let (dx, dy) = if is_horizontal {
                (1.0, 0.0)
            } else {
                (0.0, 1.0)
            };

            let original_shapes_to_kern = shapes_to_kern.clone();
            for i in 1..shapes_to_kern.len() {
                // Make sure shapes_to_kern[i] is past shapes_to_kern[i-1]
                // Shipping can cause this ^
                let mut distances_kerened = 0.;
                if is_horizontal {
                    let dx = shapes_to_kern[i].bounding_rect().unwrap().center().x
                        - shapes_to_kern[i - 1].bounding_rect().unwrap().center().x
                        - epsilon;
                    if dx < 0.0 {
                        shapes_to_kern[i].translate_mut(-dx, 0.0);
                        distances_kerened += -dx;
                    }
                } else {
                    let dy = shapes_to_kern[i].bounding_rect().unwrap().center().y
                        - shapes_to_kern[i - 1].bounding_rect().unwrap().center().y
                        - epsilon;
                    if dy < 0.0 {
                        shapes_to_kern[i].translate_mut(0.0, -dy);
                        distances_kerened += -dy;
                    }
                }

                // kern two letters
                let mut distance = Euclidean.distance(&shapes_to_kern[i - 1], &shapes_to_kern[i]);
                if distance < space {
                    // Go over
                    let mut max = distance;
                    while distance < space {
                        let d = 1.1 * (space - distance) + epsilon;
                        max = d;
                        shapes_to_kern[i].translate_mut(d * dx, d * dy);
                        distances_kerened += d;
                        distance = Euclidean.distance(&shapes_to_kern[i - 1], &shapes_to_kern[i]);
                    }

                    // Binary search
                    let mut mid = epsilon * 2.0;
                    while (distance - space).abs() < epsilon && mid >= epsilon {
                        mid = max / 2.;
                        let new_shape = shapes_to_kern[i].translate(-mid * dx, -mid * dy);
                        distances_kerened -= mid;
                        distance = Euclidean.distance(&shapes_to_kern[i - 1], &shapes_to_kern[i]);
                        if distance > space {
                            shapes_to_kern[i] = new_shape;
                        }
                        max = mid;
                    }
                }

                // Check to see if we have to respect the distance for the rest of the letters
                if distances_kerened > 0. && i + 1 < shapes_to_kern.len() {
                    data.context
                        .register_global_property(js_string!("j"), i + 1, Attribute::all())
                        .expect("property shouldn't exist");

                    if let Ok(JsValue::Boolean(value)) =
                        data.context.eval(Source::from_bytes(&self.respect_space))
                    {
                        if value {
                            for j in i + 1..shapes_to_kern.len() {
                                shapes_to_kern[j]
                                    .translate_mut(distances_kerened * dx, distances_kerened * dy);
                            }
                        }
                    }
                }
            }

            let new_shapes_to_kern_border = MultiPolygon::new(shapes_to_kern.clone())
                .bounding_rect()
                .unwrap();
            match direction {
                Direction::Left => {
                    for shape in &mut shapes_to_kern {
                        shape.translate_mut(
                            shapes_to_kern_border.max().x - new_shapes_to_kern_border.max().x,
                            0.,
                        );
                    }
                }
                Direction::Bottom => {
                    for shape in &mut shapes_to_kern {
                        shape.translate_mut(
                            0.,
                            shapes_to_kern_border.max().y - new_shapes_to_kern_border.max().y,
                        );
                    }
                }
                Direction::Center => {
                    let dx = dx
                        * (shapes_to_kern_border.center().x - new_shapes_to_kern_border.center().x);
                    let dy = dy
                        * (shapes_to_kern_border.center().y - new_shapes_to_kern_border.center().y);
                    for shape in &mut shapes_to_kern {
                        shape.translate_mut(dx, dy);
                    }
                }
                _ => {}
            }

            let mut new_inner_shapes_additions = Vec::new();
            'inner_shapes_loop: for inner_shape_index in &inner_shapes {
                let inner_shape_index = *inner_shape_index;
                for i in 0..original_shapes_to_kern.len() {
                    if original_shapes_to_kern[i].contains(&shapes[inner_shape_index]) {
                        let og_framme_mid =
                            original_shapes_to_kern[i].bounding_rect().unwrap().center();
                        let ke_framme_mid = shapes_to_kern[i].bounding_rect().unwrap().center();

                        let dif = ke_framme_mid - og_framme_mid;
                        new_inner_shapes_additions.push((
                            inner_shape_index,
                            shapes[inner_shape_index].translate(dif.x, dif.y),
                        ));

                        continue 'inner_shapes_loop;
                    }
                }
            }

            for (index, shape) in new_inner_shapes_additions {
                inner_shapes.remove(&index);
                new_inner_shapes.push(shape);
            }

            new_group.push(shapes_to_kern);
        }

        let mut shapes = data.shapes.lock().unwrap();
        let mut group_indexes = Vec::new();
        for group in new_group {
            let mut g_index = Vec::new();

            for polygon in group {
                g_index.push(shapes.len());
                shapes.push(polygon);
            }

            group_indexes.push(g_index);
        }

        // Inner shapes adds
        let mut inner_groups = Vec::new();
        let inner_iter = inner_shapes
            .into_iter()
            .map(|index| shapes[index].clone())
            .collect::<Vec<Polygon>>()
            .into_iter()
            .chain(new_inner_shapes.into_iter());

        for shape in inner_iter {
            inner_groups.push(vec![shapes.len()]);
            shapes.push(shape);
        }

        let mut groups = data.groups.lock().unwrap();
        groups.insert(self.set_group.clone(), group_indexes);
        groups.insert(self.set_inner_shapes.clone(), inner_groups);

        Ok(())
    }
}
