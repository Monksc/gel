//! The `Nest` instruction: irregular-polygon bin packing via `jagua-rs` +
//! its `lbf` reference optimizer.
//!
//! Behind the `nest` feature, because those are git dependencies (`lbf` is
//! not published) and consumers that only run queries should not build a
//! packing engine they never call. A program containing `Nest` fails to
//! parse without the feature - `unknown variant 'Nest'` - which is the
//! failure we want: refuse the program rather than quietly produce a layout
//! with nothing nested.
//!
//! Unlike `gel-mcp`'s `nest` tool, which hands placement data back to a
//! caller to apply, this applies the placements itself and writes the placed
//! geometry into groups - one slot per sheet - so the rest of a program can
//! measure and emit sheets with the ordinary group accessors.

use geo::{Centroid, Rotate, Translate};
use jagua_rs::collision_detection::CDEConfig;
use jagua_rs::geometry::fail_fast::SPSurrogateConfig;
use jagua_rs::io::ext_repr::{ExtContainer, ExtItem, ExtShape, ExtSPolygon};
use jagua_rs::io::import::Importer;
use jagua_rs::probs::bpp::io::ext_repr::{ExtBPInstance, ExtBin, ExtItem as ExtBPItem};
use jagua_rs::probs::bpp::io::import_instance;
use lbf::config::LBFConfig;
use lbf::opt::lbf_bpp::LBFOptimizerBP;
use rand::SeedableRng;
use rand::rngs::SmallRng;
use serde::{Deserialize, Serialize};

use crate::*;

/// Packs a group's shapes onto sheets.
///
/// `set_group` receives the placed geometry, one slot per sheet, so
/// `len(set_group)` is the sheet count and `frame(set_group, n)` is what
/// landed on sheet n. `set_sheets_group`, when given, receives one rectangle
/// per sheet - the stock outline - which is what a header should anchor to,
/// since the parts alone do not reach the sheet's edges.
///
/// Note on what is deliberately absent: there is no `origin` (laser packs
/// top-left, milling bottom-left per asgdraw.txt) and no `allow_mirror`.
/// `lbf` packs from its own origin and `jagua-rs`'s item API exposes only
/// `allowed_orientations`, with no mirroring - so both would be
/// accept-and-ignore, which is worse than not offering them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nest {
    pub get_group: String,
    pub set_group: String,
    /// Optional: one rectangle per sheet, for anchoring and plate borders.
    #[serde(default)]
    pub set_sheets_group: Option<String>,
    /// Expression, in drawing units.
    pub sheet_width: String,
    /// Expression, in drawing units.
    pub sheet_height: String,
    /// Expression. Gap held between parts - kerf plus handling clearance.
    #[serde(default)]
    pub spacing: Option<String>,
    /// Orientations a part may be placed at, in degrees. Defaults to `[0]`
    /// (no rotation), matching how these sheets are cut today.
    #[serde(default)]
    pub rotations: Option<Vec<f64>>,
}

fn eval_number(data: &mut Data, expression: &str, field: &str) -> Result<f64, String> {
    use boa_engine::{JsValue, Source};
    let value = data
        .context
        .eval(Source::from_bytes(expression))
        .map_err(|err| format!("could not evaluate {field} ({expression:?}): {err}"))?;
    match value {
        JsValue::Integer(n) => Ok(n as f64),
        JsValue::Rational(n) => Ok(n),
        other => Err(format!(
            "{field} ({expression:?}) evaluated to {other:?}, which is not a number"
        )),
    }
}

impl Query for Nest {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let get_group = resolve_template(data, &self.get_group, "Nest.get_group")?;
        let set_group = resolve_template(data, &self.set_group, "Nest.set_group")?;
        let set_sheets_group = match &self.set_sheets_group {
            Some(name) => Some(resolve_template(data, name, "Nest.set_sheets_group")?),
            None => None,
        };
        let sheet_width = eval_number(data, &self.sheet_width, "Nest.sheet_width")? as f32;
        let sheet_height = eval_number(data, &self.sheet_height, "Nest.sheet_height")? as f32;
        let spacing = match &self.spacing {
            Some(expression) => Some(eval_number(data, expression, "Nest.spacing")? as f32),
            None => None,
        };
        let rotations: Vec<f32> = self
            .rotations
            .clone()
            .unwrap_or_else(|| vec![0.0])
            .into_iter()
            .map(|degrees| (degrees as f32).to_radians())
            .collect();

        // One nest item per shape index; group entries are flattened, so
        // narrowing to just the outer profiles is the program's job.
        let gel_indexes: Vec<usize> = {
            let groups = data.groups.lock().unwrap();
            let entries = groups
                .get(&get_group)
                .ok_or_else(|| format!("Nest: unknown group '{get_group}'"))?;
            entries.iter().flatten().copied().collect()
        };
        if gel_indexes.is_empty() {
            return Err(format!("Nest: group '{get_group}' has no shapes"));
        }

        let items: Vec<ExtBPItem> = {
            let shapes = data.shapes.lock().unwrap();
            let mut items = Vec::with_capacity(gel_indexes.len());
            for (jagua_id, &gel_idx) in gel_indexes.iter().enumerate() {
                let polygon = shapes
                    .get(gel_idx)
                    .ok_or_else(|| format!("Nest: shape index {gel_idx} out of range"))?;
                let points: Vec<(f32, f32)> = polygon
                    .exterior()
                    .points()
                    .map(|p| (p.x() as f32, p.y() as f32))
                    .collect();
                if points.len() < 3 {
                    return Err(format!(
                        "Nest: shape {gel_idx} has fewer than 3 exterior points"
                    ));
                }
                items.push(ExtBPItem {
                    base: ExtItem {
                        id: jagua_id as u64,
                        allowed_orientations: Some(rotations.clone()),
                        shape: ExtShape::SimplePolygon(ExtSPolygon(points)),
                        min_quality: None,
                    },
                    demand: 1,
                });
            }
            items
        };

        let ext_instance = ExtBPInstance {
            name: format!("{get_group}->{set_group}"),
            items,
            bins: vec![ExtBin {
                base: ExtContainer {
                    id: 0,
                    shape: ExtShape::Rectangle {
                        x_min: 0.0,
                        y_min: 0.0,
                        width: sheet_width,
                        height: sheet_height,
                    },
                    zones: vec![],
                },
                stock: usize::MAX,
                cost: 1,
            }],
        };

        let importer = Importer::new(
            CDEConfig {
                quadtree_depth: 5,
                cd_threshold: 16,
                item_surrogate_config: SPSurrogateConfig::none(),
            },
            None,
            spacing,
            None,
        );
        let instance = import_instance(&importer, &ext_instance).map_err(|e| e.to_string())?;

        let mut config = LBFConfig::default();
        config.min_item_separation = spacing;

        // Fixed seed: the same program over the same artwork must produce the
        // same layout, or a re-run silently renests a job already on the floor.
        let rng = SmallRng::seed_from_u64(42);
        let mut optimizer = LBFOptimizerBP::new(instance.clone(), config, rng);
        let solution = optimizer.solve();

        // Apply each placement to the real geometry. Same composition
        // `gel-mcp`'s place_groups_and_render uses: rotate about the shape's
        // own centroid, then translate so that centroid lands where the
        // packer put it.
        let mut per_sheet: Vec<Vec<geo::Polygon>> = Vec::new();
        {
            let shapes = data.shapes.lock().unwrap();
            for snapshot in solution.layout_snapshots.values() {
                let mut placed_polygons = Vec::new();
                for placed in snapshot.placed_items.values() {
                    let gel_idx = gel_indexes[placed.item_id as usize];
                    let original = &shapes[gel_idx];
                    let pivot = original
                        .centroid()
                        .ok_or_else(|| format!("Nest: shape {gel_idx} has no centroid"))?;
                    let degrees = placed.d_transf.rotation.to_degrees() as f64;
                    let tx = placed.d_transf.translation.0.into_inner() as f64;
                    let ty = placed.d_transf.translation.1.into_inner() as f64;
                    let rotated = original.rotate_around_point(degrees, pivot);
                    placed_polygons.push(rotated.translate(tx - pivot.x(), ty - pivot.y()));
                }
                per_sheet.push(placed_polygons);
            }
        }

        // One slot per sheet, so `len(group)` is the sheet count and
        // `LoopOver` walks sheets.
        {
            let mut shapes = data.shapes.lock().unwrap();
            let mut depths = data.depths.lock().unwrap();
            let mut groups = data.groups.lock().unwrap();

            groups.insert(set_group.clone(), Vec::new());
            for (sheet_index, polygons) in per_sheet.iter().enumerate() {
                write_result_slot(
                    &mut shapes,
                    &mut depths,
                    &mut groups,
                    &set_group,
                    sheet_index,
                    polygons.clone(),
                );
            }

            if let Some(sheets_group) = &set_sheets_group {
                groups.insert(sheets_group.clone(), Vec::new());
                for sheet_index in 0..per_sheet.len() {
                    let rect = geo::Polygon::new(
                        geo::LineString::from(vec![
                            (0.0, 0.0),
                            (sheet_width as f64, 0.0),
                            (sheet_width as f64, sheet_height as f64),
                            (0.0, sheet_height as f64),
                            (0.0, 0.0),
                        ]),
                        vec![],
                    );
                    write_result_slot(
                        &mut shapes,
                        &mut depths,
                        &mut groups,
                        sheets_group,
                        sheet_index,
                        vec![rect],
                    );
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use geo::polygon;

    use crate::*;

    fn unit_square_at(x: f64, y: f64) -> geo::Polygon {
        polygon![
            (x: x, y: y), (x: x + 1.0, y: y),
            (x: x + 1.0, y: y + 1.0), (x: x, y: y + 1.0)
        ]
    }

    #[test]
    fn packs_everything_onto_one_sheet_when_it_fits() {
        // Four 1x1 squares, scattered, onto a 10x10 sheet.
        let mut data = Data::from(vec![
            unit_square_at(0.0, 0.0),
            unit_square_at(20.0, 0.0),
            unit_square_at(0.0, 20.0),
            unit_square_at(20.0, 20.0),
        ]);

        data.query(vec![Instruction::Nest(Nest {
            get_group: "main".into(),
            set_group: "sheets".into(),
            set_sheets_group: Some("plates".into()),
            sheet_width: "10".into(),
            sheet_height: "10".into(),
            spacing: Some("0.125".into()),
            rotations: None,
        })])
        .expect("nest should succeed");

        let groups = data.groups.lock().unwrap();
        let sheets = &groups["sheets"];
        assert_eq!(sheets.len(), 1, "four 1x1 parts fit on one 10x10 sheet");
        assert_eq!(sheets[0].len(), 4, "every part should be placed");
        assert_eq!(groups["plates"].len(), 1, "one plate outline per sheet");
    }

    #[test]
    fn spills_onto_more_sheets_when_it_does_not_fit() {
        // Four 1x1 squares onto a 1.2x1.2 sheet: only one fits per sheet.
        let mut data = Data::from(vec![
            unit_square_at(0.0, 0.0),
            unit_square_at(20.0, 0.0),
            unit_square_at(0.0, 20.0),
            unit_square_at(20.0, 20.0),
        ]);

        data.query(vec![Instruction::Nest(Nest {
            get_group: "main".into(),
            set_group: "sheets".into(),
            set_sheets_group: None,
            sheet_width: "1.2".into(),
            sheet_height: "1.2".into(),
            spacing: None,
            rotations: None,
        })])
        .expect("nest should succeed");

        let groups = data.groups.lock().unwrap();
        assert_eq!(groups["sheets"].len(), 4, "one part per sheet");
    }

    #[test]
    fn placed_geometry_lands_inside_the_sheet() {
        // The point of applying placements rather than returning them: the
        // shapes in the result must actually sit on the stock.
        let mut data = Data::from(vec![
            unit_square_at(0.0, 0.0),
            unit_square_at(50.0, 50.0),
        ]);

        data.query(vec![Instruction::Nest(Nest {
            get_group: "main".into(),
            set_group: "sheets".into(),
            set_sheets_group: None,
            sheet_width: "10".into(),
            sheet_height: "10".into(),
            spacing: None,
            rotations: None,
        })])
        .expect("nest should succeed");

        use geo::BoundingRect;
        let groups = data.groups.lock().unwrap();
        let shapes = data.shapes.lock().unwrap();
        for slot in &groups["sheets"] {
            for &i in slot {
                let frame = shapes[i].bounding_rect().unwrap();
                assert!(
                    frame.min().x >= -1e-6 && frame.min().y >= -1e-6,
                    "part starts off the sheet at {:?}", frame.min());
                assert!(
                    frame.max().x <= 10.0 + 1e-6 && frame.max().y <= 10.0 + 1e-6,
                    "part runs past the sheet at {:?}", frame.max());
            }
        }
    }

    #[test]
    fn an_empty_group_is_an_error() {
        let mut data = Data::from(vec![unit_square_at(0.0, 0.0)]);
        let result = data.query(vec![Instruction::Nest(Nest {
            get_group: "nothing_here".into(),
            set_group: "sheets".into(),
            set_sheets_group: None,
            sheet_width: "10".into(),
            sheet_height: "10".into(),
            spacing: None,
            rotations: None,
        })]);
        assert!(result.is_err(), "nesting an unknown group should fail loudly");
    }
}
