//! `AddShape` - introduce new geometry from explicit points.
//!
//! Every other instruction derives geometry from geometry already present.
//! This is the only way a program creates a shape from nothing, which is what
//! the "loop over every part and drop a rectangle in its lower-right corner"
//! case needs.
//!
//! Points are straight segments only. A curve would mean a representation gel
//! does not have - shapes are polygons throughout, curves having been
//! flattened at import - so accepting one would be a promise the rest of the
//! pipeline cannot keep.
//!
//! Copying existing geometry is deliberately *not* here: that is `Copy` plus
//! `Transformation`, which already compose.

use geo::{BoundingRect, LineString, Polygon};
use serde::{Deserialize, Serialize};

use crate::*;

/// Builds a polygon from explicit points and writes it to a group slot.
///
/// With `anchor_group`, points are relative to a corner of that group's
/// frame; without it they are absolute. Anchoring is what makes the
/// instruction useful inside a `LoopOver` - the same point list lands
/// correctly on every part without the program knowing where any of them are.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddShape {
    pub set_group: String,
    #[serde(default)]
    pub set_index: Index,
    /// `[[x, y], ...]`, each an expression. Closed automatically; at least
    /// three points.
    pub points: Vec<[String; 2]>,
    /// Make the points relative to this group's frame.
    #[serde(default)]
    pub anchor_group: Option<String>,
    #[serde(default)]
    pub anchor_index: Index,
    /// Which corner of the anchor frame is the origin: `"bottom_left"`
    /// (default), `"bottom_right"`, `"top_left"`, `"top_right"`, `"center"`.
    #[serde(default)]
    pub corner: Option<String>,
}

impl Query for AddShape {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        if self.points.len() < 3 {
            return Err(format!(
                "AddShape.points has {} point(s); a polygon needs at least 3",
                self.points.len()
            ));
        }

        let set_group = resolve_template(data, &self.set_group, "AddShape.set_group")?;
        let set_index = self.set_index.resolve(data, "AddShape.set_index")?;

        let (origin_x, origin_y) = match &self.anchor_group {
            Some(group) => {
                let group = resolve_template(data, group, "AddShape.anchor_group")?;
                let anchor_index = self.anchor_index.resolve(data, "AddShape.anchor_index")?;
                let frame = {
                    let groups = data.groups.lock().unwrap();
                    let shapes = data.shapes.lock().unwrap();
                    let indices = resolve_indices(&groups, &group, anchor_index);
                    if indices.is_empty() {
                        return Err(format!(
                            "AddShape.anchor_group '{group}' slot {anchor_index} is empty"
                        ));
                    }
                    multipolygon_from_indices(&shapes, &indices)
                        .bounding_rect()
                        .ok_or_else(|| format!("AddShape.anchor_group '{group}' has no extent"))?
                };
                match self.corner.as_deref().unwrap_or("bottom_left") {
                    "bottom_right" => (frame.max().x, frame.min().y),
                    "top_left" => (frame.min().x, frame.max().y),
                    "top_right" => (frame.max().x, frame.max().y),
                    "center" => (
                        (frame.min().x + frame.max().x) / 2.0,
                        (frame.min().y + frame.max().y) / 2.0,
                    ),
                    "bottom_left" => (frame.min().x, frame.min().y),
                    other => {
                        return Err(format!(
                            "AddShape.corner {other:?} is not one of bottom_left, bottom_right, \
                             top_left, top_right, center"
                        ))
                    }
                }
            }
            None => (0.0, 0.0),
        };

        let mut coords = Vec::with_capacity(self.points.len());
        for (n, [x, y]) in self.points.iter().enumerate() {
            let x = eval_number(data, x, &format!("AddShape.points[{n}].x"))?;
            let y = eval_number(data, y, &format!("AddShape.points[{n}].y"))?;
            coords.push((origin_x + x, origin_y + y));
        }

        // LineString::from closes the ring itself, but only if the last point
        // isn't already the first - writing it out explicitly either way would
        // leave a zero-length segment.
        let polygon = Polygon::new(LineString::from(coords), vec![]);

        let mut shapes = data.shapes.lock().unwrap();
        let mut depths = data.depths.lock().unwrap();
        let mut groups = data.groups.lock().unwrap();
        write_result_slot(
            &mut shapes,
            &mut depths,
            &mut groups,
            &set_group,
            set_index,
            vec![polygon],
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::*;
    use geo::{Area, BoundingRect};

    fn square(x: f64, y: f64, size: f64) -> geo::Polygon<f64> {
        geo::Polygon::new(
            geo::LineString::from(vec![
                (x, y),
                (x + size, y),
                (x + size, y + size),
                (x, y + size),
                (x, y),
            ]),
            vec![],
        )
    }

    #[test]
    fn absolute_points_make_the_polygon_asked_for() {
        let mut data = Data::from(vec![square(0.0, 0.0, 1.0)]);
        AddShape {
            set_group: "box".into(),
            set_index: Index::Literal(0),
            points: vec![
                ["0".into(), "0".into()],
                ["2".into(), "0".into()],
                ["2".into(), "3".into()],
                ["0".into(), "3".into()],
            ],
            anchor_group: None,
            anchor_index: Index::Literal(0),
            corner: None,
        }
        .query(&mut data)
        .expect("should add the shape");

        let groups = data.groups.lock().unwrap();
        let shapes = data.shapes.lock().unwrap();
        let index = groups["box"][0][0];
        assert!((shapes[index].unsigned_area() - 6.0).abs() < 1e-9);
    }

    /// The case this instruction exists for: same point list, different part,
    /// lands in the right place without the program knowing any coordinates.
    #[test]
    fn anchoring_puts_the_shape_on_the_parts_corner() {
        // Two parts far apart, so a wrong anchor is obvious.
        let mut data = Data::from(vec![square(0.0, 0.0, 4.0), square(100.0, 50.0, 4.0)]);
        {
            // `Data::from` renumbers shapes as it builds the containment
            // tree, so the slot order has to come from the geometry. Writing
            // `vec![vec![0], vec![1]]` here silently tests the wrong part.
            let by_position: Vec<usize> = {
                let shapes = data.shapes.lock().unwrap();
                let mut order: Vec<usize> = (0..shapes.len()).collect();
                order.sort_by(|&a, &b| {
                    let ax = shapes[a].bounding_rect().unwrap().min().x;
                    let bx = shapes[b].bounding_rect().unwrap().min().x;
                    ax.partial_cmp(&bx).unwrap()
                });
                order
            };
            let mut groups = data.groups.lock().unwrap();
            groups.insert("parts".into(), by_position.into_iter().map(|i| vec![i]).collect());
        }
        RunCode { code: "n = 0;".into() }.query(&mut data).unwrap();

        LoopOver {
            get_group: "parts".into(),
            iterator_name: "part".into(),
            instructions: vec![
                Instruction::AddShape(AddShape {
                    set_group: "tag-{n}".into(),
                    set_index: Index::Literal(0),
                    // A half-inch tag hanging off the part's bottom-right.
                    points: vec![
                        ["-0.5".into(), "0".into()],
                        ["0".into(), "0".into()],
                        ["0".into(), "0.5".into()],
                        ["-0.5".into(), "0.5".into()],
                    ],
                    anchor_group: Some("part".into()),
                    anchor_index: Index::Literal(0),
                    corner: Some("bottom_right".into()),
                }),
                Instruction::RunCode(RunCode {
                    code: "n = n + 1;".into(),
                }),
            ],
        }
        .query(&mut data)
        .expect("loop should run");

        let groups = data.groups.lock().unwrap();
        let shapes = data.shapes.lock().unwrap();
        for (pass, expected) in [(0usize, (3.5, 4.0, 0.0)), (1, (103.5, 104.0, 50.0))] {
            let index = groups[&format!("tag-{pass}")][0][0];
            let frame = shapes[index].bounding_rect().unwrap();
            assert!((frame.min().x - expected.0).abs() < 1e-9, "pass {pass}: {frame:?}");
            assert!((frame.max().x - expected.1).abs() < 1e-9, "pass {pass}: {frame:?}");
            assert!((frame.min().y - expected.2).abs() < 1e-9, "pass {pass}: {frame:?}");
        }
    }

    #[test]
    fn too_few_points_is_an_error() {
        let mut data = Data::from(vec![square(0.0, 0.0, 1.0)]);
        let err = AddShape {
            set_group: "box".into(),
            set_index: Index::Literal(0),
            points: vec![["0".into(), "0".into()], ["1".into(), "1".into()]],
            anchor_group: None,
            anchor_index: Index::Literal(0),
            corner: None,
        }
        .query(&mut data)
        .expect_err("two points is not a polygon");
        assert!(err.contains("at least 3"), "got: {err}");
    }
}
