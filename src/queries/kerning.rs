use boa_engine::{Context, JsValue, Source, js_string, property::Attribute};
use geo::{
    BoundingRect, Centroid, Contains, Distance, Euclidean, MultiPolygon, Polygon, Translate,
};
use rstar::{AABB, RTreeObject};
use serde::{Deserialize, Serialize};

use crate::*;

struct Node<T> {
    point: geo::Point,
    value: T,
}

impl<T> RTreeObject for Node<T> {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        let p = self.point;
        AABB::from_point([p.x(), p.y()])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

fn kern_group(
    shapes_to_kern: &mut Vec<Polygon>,
    epsilon: f64,
    space: f64,
    respect_space: &str,
    is_horizontal: bool,
    direction: Direction,
    context: &mut Context,
) {
    let (dx, dy) = if is_horizontal {
        (1.0, 0.0)
    } else {
        (0.0, 1.0)
    };

    let Some(shapes_to_kern_border) = MultiPolygon::new(shapes_to_kern.clone()).bounding_rect()
    else {
        return;
    };

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
            context
                .register_global_property(js_string!("j"), i + 1, Attribute::all())
                .expect("property shouldn't exist");

            if let Ok(JsValue::Boolean(value)) = context.eval(Source::from_bytes(respect_space)) {
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

    // Properly Align. Dont need Top and Right as its correct by default
    match direction {
        Direction::Left => {
            for shape in shapes_to_kern {
                shape.translate_mut(
                    shapes_to_kern_border.max().x - new_shapes_to_kern_border.max().x,
                    0.,
                );
            }
        }
        Direction::Bottom => {
            for shape in shapes_to_kern {
                shape.translate_mut(
                    0.,
                    shapes_to_kern_border.max().y - new_shapes_to_kern_border.max().y,
                );
            }
        }
        Direction::Center => {
            let dx = dx * (shapes_to_kern_border.center().x - new_shapes_to_kern_border.center().x);
            let dy = dy * (shapes_to_kern_border.center().y - new_shapes_to_kern_border.center().y);
            for shape in shapes_to_kern {
                shape.translate_mut(dx, dy);
            }
        }
        _ => {}
    }
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

        let mut tree = rstar::RTree::bulk_load(
            kerned_group
                .iter()
                .enumerate()
                .filter_map(|(group_index, indexes)| {
                    let mut new_shapes = Vec::new();

                    for shape_index in indexes {
                        let shape = shapes[*shape_index].clone();
                        new_shapes.push(shape);
                    }

                    let Some(rect) = MultiPolygon::new(new_shapes.clone()).bounding_rect() else {
                        return None;
                    };

                    Some(Node {
                        point: rect.centroid(),
                        value: (group_index, new_shapes),
                    })
                })
                .collect::<Vec<Node<(usize, Vec<Polygon>)>>>(),
        );

        for border in &borders_group {
            let Some(bounding_rect) = MultiPolygon::new(
                border
                    .into_iter()
                    .map(|index| shapes[*index].clone())
                    .collect(),
            )
            .bounding_rect() else {
                continue;
            };

            let bbox = AABB::from_corners(
                [bounding_rect.min().x, bounding_rect.min().y],
                [bounding_rect.max().x, bounding_rect.max().y],
            );

            let mut inside: Vec<Node<(usize, Vec<Polygon>, geo::Rect, bool, Option<Direction>)>> =
                tree.drain_in_envelope(bbox)
                    .filter_map(|node| {
                        let Some(rect) = MultiPolygon::new(node.value.1.clone()).bounding_rect()
                        else {
                            return None;
                        };

                        let Some(first) = node.value.1.first() else {
                            return None;
                        };
                        let Some(center) = first.centroid() else {
                            return None;
                        };

                        let mut min_x = center.x();
                        let mut min_y = center.y();
                        let mut max_x = center.x();
                        let mut max_y = center.y();

                        for polygon in &node.value.1 {
                            let Some(center) = polygon.centroid() else {
                                continue;
                            };

                            if min_x > center.x() {
                                min_x = center.x();
                            }
                            if min_y > center.y() {
                                min_y = center.y();
                            }

                            if max_x < center.x() {
                                max_x = center.x();
                            }
                            if max_y < center.y() {
                                max_y = center.y();
                            }
                        }

                        Some(Node {
                            point: node.point,
                            value: (
                                node.value.0,
                                node.value.1,
                                rect,
                                (max_x - min_x) >= (max_y - min_y),
                                None,
                            ),
                        })
                    })
                    .collect();

            // Calculating Relative Orientation
            for i in 0..inside.len() {
                for j in (i + 1)..inside.len() {
                    // Dont reupdate
                    if inside[j].value.4.is_some() {
                        continue;
                    }

                    // Different orientations
                    if inside[i].value.3 != inside[j].value.3 {
                        continue;
                    }

                    let is_horizontal = inside[i].value.3;

                    if is_horizontal {
                        // Check for right
                        if (inside[i].value.2.min().x - inside[j].value.2.min().x).abs() < 0.1 {
                            inside[i].value.4 = Some(Direction::Right);
                            inside[j].value.4 = Some(Direction::Right);
                            break;
                        }

                        // Check for center
                        if (inside[i].value.2.center().x - inside[j].value.2.center().x).abs() < 0.1
                        {
                            inside[i].value.4 = Some(Direction::Center);
                            inside[j].value.4 = Some(Direction::Center);
                            break;
                        }

                        // Check for left
                        if (inside[i].value.2.max().x - inside[j].value.2.max().x).abs() < 0.1 {
                            inside[i].value.4 = Some(Direction::Left);
                            inside[j].value.4 = Some(Direction::Left);
                            break;
                        }
                    } else {
                        // Check for top
                        if (inside[i].value.2.min().y - inside[j].value.2.min().y).abs() < 0.1 {
                            inside[i].value.4 = Some(Direction::Top);
                            inside[j].value.4 = Some(Direction::Top);
                            break;
                        }

                        // Check for center
                        if (inside[i].value.2.center().y - inside[j].value.2.center().y).abs() < 0.1
                        {
                            inside[i].value.4 = Some(Direction::Center);
                            inside[j].value.4 = Some(Direction::Center);
                            break;
                        }

                        // Check for Bottom
                        if (inside[i].value.2.max().x - inside[j].value.2.max().y).abs() < 0.1 {
                            inside[i].value.4 = Some(Direction::Bottom);
                            inside[j].value.4 = Some(Direction::Bottom);
                            break;
                        }
                    }
                }

                if inside[i].value.4.is_some() {
                    continue;
                }

                if inside[i].value.3 {
                    let l = inside[i].value.2.min().x - bounding_rect.min().x;
                    let r = bounding_rect.max().x - inside[i].value.2.max().x;

                    if (l - r).abs() < 0.1 {
                        inside[i].value.4 = Some(Direction::Center);
                    } else if l > 2. * r {
                        inside[i].value.4 = Some(Direction::Left);
                    } else if r > 1.1 * l || l - 0.5 < inside[i].value.2.height() {
                        inside[i].value.4 = Some(Direction::Right);
                    } else {
                        inside[i].value.4 = Some(Direction::Center);
                    }
                } else {
                    let u = inside[i].value.2.min().y - bounding_rect.min().y;
                    let b = bounding_rect.max().y - inside[i].value.2.max().y;

                    if (b - u).abs() < 0.1 {
                        inside[i].value.4 = Some(Direction::Center);
                    } else if b > 2. * u {
                        inside[i].value.4 = Some(Direction::Top);
                    } else if u > 1.1 * b || b - 0.5 < inside[i].value.2.height() {
                        inside[i].value.4 = Some(Direction::Bottom);
                    } else {
                        inside[i].value.4 = Some(Direction::Center);
                    }
                }
            }

            for mut node in inside {
                data.context
                    .register_global_property(js_string!("i"), node.value.0, Attribute::all())
                    .expect("property shouldn't exist");

                if node.value.1.len() < 2 {
                    continue;
                }
                let is_horizontal = node.value.3;
                if is_horizontal {
                    node.value.1.sort_by(|l, r| {
                        l.bounding_rect()
                            .unwrap()
                            .min()
                            .x
                            .partial_cmp(&r.bounding_rect().unwrap().min().x)
                            .unwrap()
                    });
                } else {
                    node.value.1.sort_by(|l, r| {
                        l.bounding_rect()
                            .unwrap()
                            .min()
                            .y
                            .partial_cmp(&r.bounding_rect().unwrap().min().y)
                            .unwrap()
                    });
                }

                let Some(direction) = node.value.4 else {
                    continue;
                };

                let original_shapes_to_kern = node.value.1.clone();

                kern_group(
                    &mut node.value.1,
                    epsilon,
                    space,
                    &self.respect_space,
                    node.value.3,
                    direction,
                    &mut data.context,
                );

                let mut new_inner_shapes_additions = Vec::new();
                'inner_shapes_loop: for inner_shape_index in &inner_shapes {
                    let inner_shape_index = *inner_shape_index;
                    for i in 0..original_shapes_to_kern.len() {
                        if original_shapes_to_kern[i].contains(&shapes[inner_shape_index]) {
                            let og_framme_mid =
                                original_shapes_to_kern[i].bounding_rect().unwrap().center();
                            let ke_framme_mid = node.value.1[i].bounding_rect().unwrap().center();

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

                new_group.push(node.value.1);
            }
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
