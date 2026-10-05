//! Irregular-polygon nesting/bin-packing, via `jagua-rs` + its `lbf`
//! reference optimizer - see DESIGN.md's hands-on evaluation
//! (`~/Projects/rust/eval/nest_example`) for how this was chosen and
//! verified (irregular shapes, `RotationRange::None`, `min_item_separation`
//! for kerf/spacing, and fixed-max-sheet-size via the bin-packing mode
//! used here specifically).
//!
//! Deliberately does NOT reconstruct placed-shape geometry by hand
//! (rotating/translating the original `geo::Polygon`s ourselves) - that's
//! real transform math with real risk of a subtle bug, and `lbf` already
//! has a correct SVG renderer for its own solutions
//! (`jagua_rs::io::svg::s_layout_to_svg`). This just calls that directly
//! and hands back the resulting file paths plus the raw placement data
//! (translation/rotation), rather than re-deriving the geometry.
//!
//! One nest item per shape *index* in the requested group (group entries
//! are flattened - if the caller wants only outer sign profiles nested,
//! filtering down to just those indices before calling this is the
//! caller's job, same as every other tool here being a thin wrapper).

use std::collections::HashMap;
use std::path::PathBuf;

use gel::Data;
use jagua_rs::collision_detection::CDEConfig;
use jagua_rs::geometry::fail_fast::SPSurrogateConfig;
use jagua_rs::io::ext_repr::{ExtContainer, ExtItem, ExtShape, ExtSPolygon};
use jagua_rs::io::import::Importer;
use jagua_rs::io::svg::s_layout_to_svg;
use jagua_rs::probs::bpp::io::ext_repr::{ExtBPInstance, ExtBin, ExtItem as ExtBPItem};
use jagua_rs::probs::bpp::io::import_instance;
use lbf::config::LBFConfig;
use lbf::opt::lbf_bpp::LBFOptimizerBP;
use rand::SeedableRng;
use rand::rngs::SmallRng;
use serde_json::{Value, json};

pub fn nest(
    handles: &HashMap<String, Data>,
    handle: &str,
    group: &str,
    params: &Value,
) -> Result<Value, String> {
    let data = handles.get(handle).ok_or_else(|| format!("unknown handle '{handle}'"))?;

    let sheet_width = params.get("sheet_width").and_then(Value::as_f64)
        .ok_or("sheet_width (number) is required")? as f32;
    let sheet_height = params.get("sheet_height").and_then(Value::as_f64)
        .ok_or("sheet_height (number) is required")? as f32;
    let min_item_separation = params.get("min_item_separation").and_then(Value::as_f64).map(|v| v as f32);
    let allow_rotation = params.get("allow_rotation").and_then(Value::as_bool).unwrap_or(false);
    let out_dir = params.get("out_dir").and_then(Value::as_str)
        .ok_or("out_dir (directory path to write SVGs into) is required")?;

    // Flatten the group's entries into one shape-index-per-nest-item list.
    let gel_indexes: Vec<usize> = {
        let groups = data.groups.lock().unwrap();
        let entries = groups.get(group).ok_or_else(|| format!("unknown group '{group}'"))?;
        entries.iter().flatten().copied().collect()
    };
    if gel_indexes.is_empty() {
        return Err(format!("group '{group}' has no shapes to nest"));
    }

    let items: Result<Vec<ExtBPItem>, String> = {
        let shapes = data.shapes.lock().unwrap();
        gel_indexes
            .iter()
            .enumerate()
            .map(|(jagua_id, &gel_idx)| {
                let polygon = shapes.get(gel_idx).ok_or_else(|| format!("shape index {gel_idx} out of range"))?;
                let points: Vec<(f32, f32)> = polygon
                    .exterior()
                    .points()
                    .map(|p| (p.x() as f32, p.y() as f32))
                    .collect();
                if points.len() < 3 {
                    return Err(format!("shape index {gel_idx} has fewer than 3 exterior points, cannot nest"));
                }
                Ok(ExtBPItem {
                    base: ExtItem {
                        id: jagua_id as u64,
                        allowed_orientations: if allow_rotation { None } else { Some(vec![0.0]) },
                        shape: ExtShape::SimplePolygon(ExtSPolygon(points)),
                        min_quality: None,
                    },
                    demand: 1,
                })
            })
            .collect()
    };
    let items = items?;

    let ext_instance = ExtBPInstance {
        name: format!("{handle}:{group}"),
        items,
        bins: vec![ExtBin {
            base: ExtContainer {
                id: 0,
                shape: ExtShape::Rectangle { x_min: 0.0, y_min: 0.0, width: sheet_width, height: sheet_height },
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
        min_item_separation,
        None,
    );

    let instance = import_instance(&importer, &ext_instance).map_err(|e| e.to_string())?;

    let mut config = LBFConfig::default();
    config.min_item_separation = min_item_separation;

    let rng = SmallRng::seed_from_u64(42);
    let mut optimizer = LBFOptimizerBP::new(instance.clone(), config, rng);
    let solution = optimizer.solve();

    std::fs::create_dir_all(out_dir).map_err(|e| format!("could not create out_dir: {e}"))?;

    let mut sheets = Vec::new();
    for (sheet_idx, snapshot) in solution.layout_snapshots.values().enumerate() {
        let svg = s_layout_to_svg(snapshot, &instance, config.svg_draw_options, "");
        let svg_path: PathBuf = PathBuf::from(out_dir).join(format!("sheet_{sheet_idx}.svg"));
        svg::save(&svg_path, &svg).map_err(|e| format!("could not write SVG: {e}"))?;

        let placements: Vec<Value> = snapshot
            .placed_items
            .values()
            .map(|placed| {
                let gel_index = gel_indexes[placed.item_id];
                json!({
                    "gel_shape_index": gel_index,
                    "translation": [placed.d_transf.translation.0.into_inner(), placed.d_transf.translation.1.into_inner()],
                    "rotation_degrees": placed.d_transf.rotation.to_degrees(),
                })
            })
            .collect();

        sheets.push(json!({
            "svg_path": svg_path.to_string_lossy(),
            "items_placed": placements.len(),
            "placements": placements,
        }));
    }

    Ok(json!({
        "sheet_width": sheet_width,
        "sheet_height": sheet_height,
        "sheets_used": sheets.len(),
        "density": solution.density(&instance),
        "sheets": sheets,
    }))
}
