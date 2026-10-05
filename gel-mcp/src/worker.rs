//! Owns every loaded `gel::Data` on one dedicated OS thread.
//!
//! `boa_engine::Context` (which `gel::Data` embeds for JS predicate eval)
//! is not `Send`, so it can never hop across a tokio worker thread. Rather
//! than fight that, every `Data` lives and dies on this one thread; the
//! async MCP tool handlers talk to it over a channel and just await the
//! reply. This is the same reason this crate isn't itself invoking gel's
//! Filter/GroupBy/Sort concurrently across multiple files - one queue, one
//! thread, no shared-Data races.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender as StdSender, channel};
use tokio::sync::oneshot::Sender as Reply;

use gel::{Data, Filter, GroupBy, Query, Sort};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One group's placement, in exactly the terms `nest`/`shape_info` already
/// report: rotate `group`'s shapes around (pivot_x, pivot_y) - the
/// reference shape's own centroid - by `rotation_degrees`, then translate
/// by `translation` (nest's reported final position). See jagua-rs's
/// `centering_transformation` + `DTransformation` for why this is the
/// exact inverse of its internal placement math: it centers a shape
/// (translate centroid -> origin), then applies rotate-then-translate: so
/// for an original point P, final = R(rotation)*(P - pivot) + translation.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct PlacementEntry {
    pub group: String,
    pub pivot_x: f64,
    pub pivot_y: f64,
    pub rotation_degrees: f64,
    pub translation: (f64, f64),
}

pub enum Command {
    /// Load an SVG file into a new handle. CDR files must be converted to
    /// SVG before reaching this (same LibreOffice step asg-quote-cost's
    /// cdrConverter.js already uses) - gel only reads SVG.
    LoadSvg { path: String, respond: Reply<Result<String, String>> },
    /// Run a boa JS predicate (same `depth(i)`, `area(i)`, `circle_metrics(i)`,
    /// `distance(...)`, `center(i)` globals gel's Filter/GroupBy already
    /// expose) over `get_group`, writing the surviving indexes into
    /// `set_group`. Read-only from the caller's perspective except for that
    /// one new/overwritten group.
    Filter { handle: String, get_group: String, set_group: String, code: String, respond: Reply<Result<Value, String>> },
    /// Same predicate style as Filter, but merges shapes pairwise into
    /// existing groups (gel's GroupBy) - e.g. proximity clustering.
    GroupBy { handle: String, get_group: String, set_group: String, code: String, respond: Reply<Result<Value, String>> },
    /// Orders a group by a JS comparator using the same `l`/`r` index globals
    /// gel's Sort already exposes.
    Sort { handle: String, get_group: String, set_group: String, compare: String, respond: Reply<Result<Value, String>> },
    /// Summary stats for a group: shape count, depth histogram, total area,
    /// combined bounding box - the "feel for the file" overview.
    Stats { handle: String, group: String, respond: Reply<Result<Value, String>> },
    /// Per-shape detail (depth, area, bbox, point count) for one group.
    ListShapes { handle: String, group: String, limit: usize, respond: Reply<Result<Value, String>> },
    /// Nests a group's shapes (one nest item per shape index, group
    /// entries flattened) onto sheet_width x sheet_height sheets via
    /// jagua-rs + lbf - see nest.rs for the real implementation and
    /// DESIGN.md for how this crate was chosen/verified.
    Nest { handle: String, group: String, params: Value, respond: Reply<Result<Value, String>> },
    /// Renders a group to an SVG file on disk via `gel::save_svg`, so
    /// Claude can `Read` it as an actual image - the "look at it" half of
    /// the two-tier visualization strategy (DESIGN.md §2), used when the
    /// numeric layer alone can't resolve a question.
    RenderGroup { handle: String, group: String, out_path: String, respond: Reply<Result<Value, String>> },
    /// Strips synthetic artifact elements (LibreOffice's per-shape
    /// `class="BoundingBox"` helper rectangles by default, or any custom
    /// attr=value rules) from a raw SVG file, writing the cleaned result
    /// to a new file - see gel::svg_filter. Meant to run between "convert
    /// CDR to SVG" and load_svg, not folded into either.
    FilterSvg { in_path: String, out_path: String, rules: Vec<(String, String)>, respond: Reply<Result<Value, String>> },
    /// Per-shape centroid/bbox/area, as plain numbers (not just JS-predicate
    /// globals) - needed to compute the offset between a shape's original
    /// position and its nested position (see `nest`'s docs on why that's a
    /// pure-translation `nest_translation - centroid` when rotation is off).
    ShapeInfo { handle: String, index: usize, respond: Reply<Result<Value, String>> },
    /// Translates every shape in a group by (dx, dy) and renders the result
    /// to an SVG file - the mechanism for "put this sign's full content at
    /// the same place its nested border ended up," using `geo::Translate`
    /// (well-tested library code) rather than hand-rolled transform math.
    TranslateAndRender { handle: String, group: String, dx: f64, dy: f64, out_path: String, respond: Reply<Result<Value, String>> },
    /// Same idea as TranslateAndRender, but for combining several groups
    /// (each with its own dx/dy) into ONE rendered sheet - e.g. several
    /// signs' full content, each placed at wherever its own nested border
    /// ended up, combined onto one milled sheet matching one laser sheet.
    TranslateGroupsAndRender { handle: String, translations: Vec<(String, f64, f64)>, sheet_borders: Vec<(f64, f64, f64, f64)>, out_path: String, respond: Reply<Result<Value, String>> },
    /// General version of TranslateGroupsAndRender that also handles
    /// rotation, taking exactly the fields `nest` reports
    /// (rotation_degrees, translation) plus the pivot to rotate around
    /// (the reference shape's own centroid, from `shape_info`) - so a
    /// caller places shapes using jagua-rs's own placement data directly,
    /// with no manual dx/dy math and no dependency on jagua's own SVG
    /// renderer/theme. See PlacementEntry for the math (reduces to plain
    /// translation when rotation_degrees is 0, same as before).
    PlaceGroupsAndRender {
        handle: String,
        placements: Vec<PlacementEntry>,
        /// Plain rectangles (offset_x, offset_y, width, height), one per
        /// physical plate, drawn alongside the placed shapes - matching the
        /// sheet boundary jagua-rs's own renderer always shows (its
        /// container), and letting several plates share ONE output file
        /// (each at its own offset) instead of one file per plate.
        sheet_borders: Vec<(f64, f64, f64, f64)>,
        out_path: String,
        respond: Reply<Result<Value, String>>,
    },
    /// Same as TranslateGroupsAndRender, but preserves each shape's real
    /// fill/stroke color instead of the forced black-hairline-outline look
    /// - for a "what does the finished sign actually look like" render
    /// (the Display/All layout) rather than a manufacturing/toolpath one.
    TranslateGroupsAndRenderStyled {
        handle: String,
        translations: Vec<(String, f64, f64)>,
        sheet_borders: Vec<(f64, f64, f64, f64)>,
        out_path: String,
        respond: Reply<Result<Value, String>>,
    },
}

pub struct WorkerHandle {
    tx: StdSender<Command>,
}

impl WorkerHandle {
    pub fn spawn() -> Self {
        let (tx, rx): (StdSender<Command>, Receiver<Command>) = channel();
        std::thread::Builder::new()
            .name("gel-data-owner".into())
            .spawn(move || run(rx))
            .expect("failed to spawn gel worker thread");
        Self { tx }
    }

    pub fn send(&self, cmd: Command) {
        // The worker thread only ever exits if the process is shutting down,
        // so a failed send here means there is nothing left to answer anyway.
        let _ = self.tx.send(cmd);
    }
}

fn run(rx: Receiver<Command>) {
    let mut handles: HashMap<String, Data> = HashMap::new();

    for cmd in rx {
        match cmd {
            Command::LoadSvg { path, respond } => {
                let _ = respond.send(load_svg(&mut handles, &path));
            }
            Command::Filter { handle, get_group, set_group, code, respond } => {
                let _ = respond.send(run_filter(&mut handles, &handle, &get_group, &set_group, &code));
            }
            Command::GroupBy { handle, get_group, set_group, code, respond } => {
                let _ = respond.send(run_groupby(&mut handles, &handle, &get_group, &set_group, &code));
            }
            Command::Sort { handle, get_group, set_group, compare, respond } => {
                let _ = respond.send(run_sort(&mut handles, &handle, &get_group, &set_group, &compare));
            }
            Command::Stats { handle, group, respond } => {
                let _ = respond.send(run_stats(&handles, &handle, &group));
            }
            Command::ListShapes { handle, group, limit, respond } => {
                let _ = respond.send(run_list_shapes(&handles, &handle, &group, limit));
            }
            Command::Nest { handle, group, params, respond } => {
                let _ = respond.send(crate::nest::nest(&handles, &handle, &group, &params));
            }
            Command::RenderGroup { handle, group, out_path, respond } => {
                let _ = respond.send(run_render_group(&handles, &handle, &group, &out_path));
            }
            Command::FilterSvg { in_path, out_path, rules, respond } => {
                let _ = respond.send(run_filter_svg(&in_path, &out_path, &rules));
            }
            Command::ShapeInfo { handle, index, respond } => {
                let _ = respond.send(run_shape_info(&handles, &handle, index));
            }
            Command::TranslateAndRender { handle, group, dx, dy, out_path, respond } => {
                let _ = respond.send(run_translate_and_render(&handles, &handle, &group, dx, dy, &out_path));
            }
            Command::TranslateGroupsAndRender { handle, translations, sheet_borders, out_path, respond } => {
                let _ = respond.send(run_translate_groups_and_render(&handles, &handle, &translations, &sheet_borders, &out_path));
            }
            Command::PlaceGroupsAndRender { handle, placements, sheet_borders, out_path, respond } => {
                let _ = respond.send(run_place_groups_and_render(&handles, &handle, &placements, &sheet_borders, &out_path));
            }
            Command::TranslateGroupsAndRenderStyled { handle, translations, sheet_borders, out_path, respond } => {
                let _ = respond.send(run_translate_groups_and_render_styled(&handles, &handle, &translations, &sheet_borders, &out_path));
            }
        }
    }
}

fn get_data<'a>(handles: &'a HashMap<String, Data>, handle: &str) -> Result<&'a Data, String> {
    handles.get(handle).ok_or_else(|| format!("unknown handle '{handle}' - call load_svg first"))
}

fn load_svg(handles: &mut HashMap<String, Data>, path: &str) -> Result<String, String> {
    let path = Path::new(path);
    if !path.exists() {
        return Err(format!("file does not exist: {}", path.display()));
    }
    let data = Data::from_svg_path_with_style(path);
    let handle = uuid::Uuid::new_v4().to_string();
    let shape_count = data.shapes.lock().unwrap().len();
    handles.insert(handle.clone(), data);
    Ok(format!("loaded '{}' as handle {handle} ({shape_count} shapes in group \"main\")", path.display()))
}

fn run_filter(handles: &mut HashMap<String, Data>, handle: &str, get_group: &str, set_group: &str, code: &str) -> Result<Value, String> {
    let data = handles.get_mut(handle).ok_or_else(|| format!("unknown handle '{handle}'"))?;
    let mut filter = Filter { set_group: set_group.into(), get_group: get_group.into(), code: code.into() };
    filter.query(data)?;
    let groups = data.groups.lock().unwrap();
    let count = groups.get(set_group).map(|g| g.len()).unwrap_or(0);
    Ok(json!({"set_group": set_group, "matched": count}))
}

fn run_groupby(handles: &mut HashMap<String, Data>, handle: &str, get_group: &str, set_group: &str, code: &str) -> Result<Value, String> {
    let data = handles.get_mut(handle).ok_or_else(|| format!("unknown handle '{handle}'"))?;
    let mut groupby = GroupBy { set_group: set_group.into(), get_group: get_group.into(), code: code.into() };
    groupby.query(data)?;
    let groups = data.groups.lock().unwrap();
    let clusters = groups.get(set_group).map(|g| g.len()).unwrap_or(0);
    Ok(json!({"set_group": set_group, "clusters": clusters}))
}

fn run_sort(handles: &mut HashMap<String, Data>, handle: &str, get_group: &str, set_group: &str, compare: &str) -> Result<Value, String> {
    let data = handles.get_mut(handle).ok_or_else(|| format!("unknown handle '{handle}'"))?;
    let mut sort = Sort { set_group: set_group.into(), get_group: get_group.into(), compare: compare.into() };
    sort.query(data)?;
    Ok(json!({"set_group": set_group}))
}

#[derive(Serialize)]
struct ShapeSummary {
    index: usize,
    depth: usize,
    area: f64,
}

fn run_stats(handles: &HashMap<String, Data>, handle: &str, group: &str) -> Result<Value, String> {
    let data = get_data(handles, handle)?;
    let groups = data.groups.lock().unwrap();
    let shape_group = groups.get(group).ok_or_else(|| format!("unknown group '{group}'"))?;
    let shapes = data.shapes.lock().unwrap();
    let depths = data.depths.lock().unwrap();

    let mut depth_histogram: HashMap<usize, usize> = HashMap::new();
    let mut total_area = 0.0;
    let mut count = 0usize;
    for entry in shape_group {
        for &i in entry {
            total_area += geo::Area::unsigned_area(&shapes[i]);
            *depth_histogram.entry(depths[i]).or_insert(0) += 1;
            count += 1;
        }
    }

    Ok(json!({
        "group": group,
        "shape_count": count,
        "group_entries": shape_group.len(),
        "total_area": total_area,
        "depth_histogram": depth_histogram,
    }))
}

fn run_list_shapes(handles: &HashMap<String, Data>, handle: &str, group: &str, limit: usize) -> Result<Value, String> {
    let data = get_data(handles, handle)?;
    let groups = data.groups.lock().unwrap();
    let shape_group = groups.get(group).ok_or_else(|| format!("unknown group '{group}'"))?;
    let shapes = data.shapes.lock().unwrap();
    let depths = data.depths.lock().unwrap();

    let mut out = Vec::new();
    'outer: for entry in shape_group {
        for &i in entry {
            out.push(ShapeSummary { index: i, depth: depths[i], area: geo::Area::unsigned_area(&shapes[i]) });
            if out.len() >= limit {
                break 'outer;
            }
        }
    }

    Ok(json!({"group": group, "shapes": out, "truncated": out.len() >= limit}))
}

fn run_render_group(handles: &HashMap<String, Data>, handle: &str, group: &str, out_path: &str) -> Result<Value, String> {
    let data = get_data(handles, handle)?;
    let groups = data.groups.lock().unwrap();
    let shape_group = groups.get(group).ok_or_else(|| format!("unknown group '{group}'"))?;
    let shapes = data.shapes.lock().unwrap();

    let polygons: Vec<geo::Polygon<f64>> = shape_group.iter().flatten().map(|&i| shapes[i].clone()).collect();
    if polygons.is_empty() {
        return Err(format!("group '{group}' has no shapes to render"));
    }

    let svg = gel::polygons_to_svg(&polygons);
    std::fs::write(out_path, svg).map_err(|e| format!("could not write SVG to {out_path}: {e}"))?;

    Ok(json!({"group": group, "shape_count": polygons.len(), "svg_path": out_path}))
}

fn run_filter_svg(in_path: &str, out_path: &str, rules: &[(String, String)]) -> Result<Value, String> {
    let svg = std::fs::read_to_string(in_path).map_err(|e| format!("could not read {in_path}: {e}"))?;
    let strip_rules: Vec<gel::StripRule> = if rules.is_empty() {
        gel::known_artifact_rules()
    } else {
        rules.iter().map(|(attr, value)| gel::StripRule::new(attr.clone(), value.clone())).collect()
    };
    let result = gel::strip_elements(&svg, &strip_rules)?;
    std::fs::write(out_path, &result.svg).map_err(|e| format!("could not write {out_path}: {e}"))?;
    Ok(json!({"out_path": out_path, "elements_stripped": result.elements_stripped}))
}

fn run_shape_info(handles: &HashMap<String, Data>, handle: &str, index: usize) -> Result<Value, String> {
    let data = get_data(handles, handle)?;
    let shapes = data.shapes.lock().unwrap();
    let polygon = shapes.get(index).ok_or_else(|| format!("shape index {index} out of range"))?;

    let centroid = geo::Centroid::centroid(polygon).ok_or_else(|| format!("shape {index} has no centroid"))?;
    let bbox = geo::BoundingRect::bounding_rect(polygon).ok_or_else(|| format!("shape {index} has no bounding rect"))?;

    Ok(json!({
        "index": index,
        "area": geo::Area::unsigned_area(polygon),
        "centroid": [centroid.x(), centroid.y()],
        "frame": {
            "min_x": bbox.min().x, "min_y": bbox.min().y,
            "max_x": bbox.max().x, "max_y": bbox.max().y,
            "width": bbox.width(), "height": bbox.height(),
        },
    }))
}

fn run_translate_and_render(
    handles: &HashMap<String, Data>,
    handle: &str,
    group: &str,
    dx: f64,
    dy: f64,
    out_path: &str,
) -> Result<Value, String> {
    let data = get_data(handles, handle)?;
    let groups = data.groups.lock().unwrap();
    let shape_group = groups.get(group).ok_or_else(|| format!("unknown group '{group}'"))?;
    let shapes = data.shapes.lock().unwrap();

    let polygons: Vec<geo::Polygon<f64>> = shape_group
        .iter()
        .flatten()
        .map(|&i| geo::Translate::translate(&shapes[i], dx, dy))
        .collect();
    if polygons.is_empty() {
        return Err(format!("group '{group}' has no shapes to render"));
    }

    let svg = gel::polygons_to_svg(&polygons);
    std::fs::write(out_path, svg).map_err(|e| format!("could not write SVG to {out_path}: {e}"))?;

    Ok(json!({"group": group, "shape_count": polygons.len(), "dx": dx, "dy": dy, "svg_path": out_path}))
}

fn run_translate_groups_and_render(
    handles: &HashMap<String, Data>,
    handle: &str,
    translations: &[(String, f64, f64)],
    sheet_borders: &[(f64, f64, f64, f64)],
    out_path: &str,
) -> Result<Value, String> {
    let data = get_data(handles, handle)?;
    let groups = data.groups.lock().unwrap();
    let shapes = data.shapes.lock().unwrap();

    let mut polygons: Vec<geo::Polygon<f64>> = Vec::new();

    for &(ox, oy, w, h) in sheet_borders {
        let border = geo::Polygon::new(
            geo::LineString::from(vec![
                (ox, oy), (ox + w, oy), (ox + w, oy + h), (ox, oy + h), (ox, oy),
            ]),
            vec![],
        );
        polygons.push(border);
    }

    let mut per_group_counts = Vec::new();
    for (group, dx, dy) in translations {
        let shape_group = groups.get(group).ok_or_else(|| format!("unknown group '{group}'"))?;
        let before = polygons.len();
        polygons.extend(
            shape_group.iter().flatten().map(|&i| geo::Translate::translate(&shapes[i], *dx, *dy)),
        );
        per_group_counts.push(json!({"group": group, "dx": dx, "dy": dy, "shape_count": polygons.len() - before}));
    }
    if polygons.is_empty() {
        return Err("no shapes to render across the given groups".into());
    }

    let svg = gel::polygons_to_svg(&polygons);
    std::fs::write(out_path, svg).map_err(|e| format!("could not write SVG to {out_path}: {e}"))?;

    Ok(json!({"svg_path": out_path, "total_shape_count": polygons.len(), "groups": per_group_counts}))
}

/// Same as `run_translate_groups_and_render`, but preserves each shape's
/// real fill/stroke color instead of forcing the black-hairline-outline
/// look - for a "what does the finished sign actually look like" render
/// (the Display/All layout) rather than a manufacturing/toolpath one.
/// Sheet-border rectangles are still drawn outline-only (`fill=none
/// stroke=black`), since they're not part of the sign artwork itself.
fn run_translate_groups_and_render_styled(
    handles: &HashMap<String, Data>,
    handle: &str,
    translations: &[(String, f64, f64)],
    sheet_borders: &[(f64, f64, f64, f64)],
    out_path: &str,
) -> Result<Value, String> {
    let data = get_data(handles, handle)?;
    let groups = data.groups.lock().unwrap();
    let shapes = data.shapes.lock().unwrap();
    let shape_data = data.shape_data.lock().unwrap();

    // Inkscape's PNG export renders `stroke="rgba(...)"` as nothing at all
    // (confirmed with an isolated minimal-SVG test - `fill="rgba(...)"`
    // works fine, only `stroke` is affected, regardless of color or alpha
    // value), which was silently dropping the icon/bar/border in this
    // layout since they're all fill:none, stroke-only shapes. Plain
    // `rgb(r,g,b)` renders correctly, so use that for the fully-opaque
    // case (everything in practice so far) and only fall back to rgba()
    // for genuine partial transparency.
    fn css_color(stored: Option<&str>) -> Option<String> {
        // gel stores paint as its own "rgba(r,g,b,a)" string with a 0-255
        // alpha (see rgba_to_js_string), which is not valid CSS.
        gel::parse_color(stored?).ok().flatten().map(|(r, g, b, a)| {
            if a == 255 {
                format!("rgb({r},{g},{b})")
            } else {
                format!("rgba({r},{g},{b},{})", a as f64 / 255.0)
            }
        })
    }

    let mut polygons: Vec<geo::Polygon<f64>> = Vec::new();
    let mut styles: Vec<gel::PathStyle> = Vec::new();

    for &(ox, oy, w, h) in sheet_borders {
        let border = geo::Polygon::new(
            geo::LineString::from(vec![
                (ox, oy), (ox + w, oy), (ox + w, oy + h), (ox, oy + h), (ox, oy),
            ]),
            vec![],
        );
        polygons.push(border);
        // Colors in this shop's CDR convention are process/layer markers
        // (e.g. white or green content, no dark backer anywhere in the
        // file), not necessarily the finished product's real appearance -
        // a light neutral gray plate fill keeps both white and near-black
        // content visible regardless of what a shape's true final color
        // is meant to be, rather than losing anything to a same-color
        // background.
        styles.push(gel::PathStyle {
            fill: Some("rgb(224,224,224)".to_string()),
            stroke: Some("rgb(0,0,0)".to_string()),
            stroke_width: None,
            extra: Vec::new(),
        });
    }

    let mut per_group_counts = Vec::new();
    for (group, dx, dy) in translations {
        let shape_group = groups.get(group).ok_or_else(|| format!("unknown group '{group}'"))?;
        let before = polygons.len();
        for &i in shape_group.iter().flatten() {
            polygons.push(geo::Translate::translate(&shapes[i], *dx, *dy));
            let paint = |key: &str| {
                shape_data.get(i).and_then(|d| d.get(key)).and_then(|v| v.as_str())
            };
            styles.push(gel::PathStyle {
                fill: css_color(paint("fill")),
                stroke: css_color(paint("stroke")),
                stroke_width: None,
                extra: Vec::new(),
            });
        }
        per_group_counts.push(json!({"group": group, "dx": dx, "dy": dy, "shape_count": polygons.len() - before}));
    }
    if polygons.is_empty() {
        return Err("no shapes to render across the given groups".into());
    }

    let svg = gel::polygons_to_svg_styled(&polygons, &styles);
    std::fs::write(out_path, svg).map_err(|e| format!("could not write SVG to {out_path}: {e}"))?;

    Ok(json!({"svg_path": out_path, "total_shape_count": polygons.len(), "groups": per_group_counts}))
}

fn run_place_groups_and_render(
    handles: &HashMap<String, Data>,
    handle: &str,
    placements: &[PlacementEntry],
    sheet_borders: &[(f64, f64, f64, f64)],
    out_path: &str,
) -> Result<Value, String> {
    let data = get_data(handles, handle)?;
    let groups = data.groups.lock().unwrap();
    let shapes = data.shapes.lock().unwrap();

    let mut polygons: Vec<geo::Polygon<f64>> = Vec::new();

    // Plain sheet-boundary rectangles, one per physical plate, each at its
    // own (offset_x, offset_y) - so several plates can share ONE output
    // file (e.g. stacked vertically) instead of one file per plate, while
    // still showing each plate's own boundary the way jagua-rs's own
    // renderer always does, instead of just auto-cropping to shape content.
    for &(ox, oy, w, h) in sheet_borders {
        let border = geo::Polygon::new(
            geo::LineString::from(vec![
                (ox, oy), (ox + w, oy), (ox + w, oy + h), (ox, oy + h), (ox, oy),
            ]),
            vec![],
        );
        polygons.push(border);
    }

    let mut per_group_counts = Vec::new();
    for p in placements {
        let shape_group = groups.get(&p.group).ok_or_else(|| format!("unknown group '{}'", p.group))?;
        let pivot = geo::Point::new(p.pivot_x, p.pivot_y);
        let dx = p.translation.0 - p.pivot_x;
        let dy = p.translation.1 - p.pivot_y;
        let before = polygons.len();
        polygons.extend(shape_group.iter().flatten().map(|&i| {
            let rotated = geo::Rotate::rotate_around_point(&shapes[i], p.rotation_degrees, pivot);
            geo::Translate::translate(&rotated, dx, dy)
        }));
        per_group_counts.push(json!({
            "group": p.group,
            "rotation_degrees": p.rotation_degrees,
            "translation": p.translation,
            "shape_count": polygons.len() - before,
        }));
    }
    if polygons.is_empty() {
        return Err("no shapes to render across the given placements".into());
    }

    let svg = gel::polygons_to_svg(&polygons);
    std::fs::write(out_path, svg).map_err(|e| format!("could not write SVG to {out_path}: {e}"))?;

    Ok(json!({"svg_path": out_path, "total_shape_count": polygons.len(), "placements": per_group_counts}))
}
