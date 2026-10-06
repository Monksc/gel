use boa_engine::{
    Context, JsError, JsResult, JsValue, NativeFunction, js_string, object::ObjectInitializer,
    property::Attribute,
};
use depth_tree::Tree;
use geo::*;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::Query;

#[derive(Debug, Default)]
pub struct Data {
    pub shapes: Arc<Mutex<Vec<Polygon>>>,
    pub depths: Arc<Mutex<Vec<usize>>>,
    pub groups: Arc<Mutex<HashMap<String, Vec<Vec<usize>>>>>,
    pub context: Context,
    /// Arbitrary per-shape data, one map per shape, set by `SetData` and read
    /// by `Emit`.
    ///
    /// Deliberately untyped. Paint used to be four hardcoded vectors
    /// (`fill_rgba`, `stroke_rgba`, `stroke_width`, `visible`), which meant a
    /// new attribute needed a new Rust field, a new instruction and a new
    /// `Emit` branch. Milling wants tool number, feed rate and depth; 3-D
    /// print wants orientation and supports; none of those are paint. One
    /// open map covers all of it, and `Emit` interprets the keys it knows.
    ///
    /// Keys `Emit` understands: `fill`, `stroke`, `stroke_width`, `visible`.
    /// Everything else is written out as a `data-*` attribute so downstream
    /// tools can read it. Populated from the source SVG when loaded via
    /// [`Data::from_svg_path_with_style`].
    pub shape_data: Arc<Mutex<Vec<serde_json::Map<String, serde_json::Value>>>>,
    /// Text labels for the operator, recorded by `AddText`.
    ///
    /// Deliberately NOT shapes. A label is never cut, so making it geometry
    /// would mean carrying glyph outlines - a font engine - for something
    /// the SVG viewer can render from a `<text>` element. `Emit` writes
    /// these only when asked (the "verbose" copy of a layout), so the cut
    /// file and the annotated file come from one program over the same
    /// geometry.
    pub annotations: Arc<Mutex<Vec<Annotation>>>,
    /// Named instruction sequences registered by `Define` and run by `Call`.
    ///
    /// Deliberately no file loading: the quote generator concatenates the
    /// shared library onto each layout program before upload, so what runs on
    /// the server is one self-contained file. That keeps the server off the
    /// filesystem and makes the composed program an exact, re-runnable record
    /// of the job.
    pub functions: Arc<Mutex<HashMap<String, FunctionDef>>>,
    /// Guards runaway recursion in `Call`.
    pub call_depth: Arc<Mutex<usize>>,
    /// Warnings collected by `Assert` in production mode.
    ///
    /// Read by the caller after the run and surfaced to whoever asked for the
    /// layout. See [`Data::debug`] for why they are not errors.
    pub warnings: Arc<Mutex<Vec<Warning>>>,
    /// Debug mode: a failed `Assert` halts the program instead of being
    /// collected.
    ///
    /// Defaults to **false (production)**, deliberately. If this defaulted to
    /// debug and a caller forgot to set it, a job with one questionable part
    /// would produce nothing at all on AWS - and a laser job with one flagged
    /// part is still worth cutting, because the operator can look at the part.
    /// Forgetting it the other way only costs a louder local failure.
    pub debug: bool,
    /// The group `Filter` / `Sort` / `GroupBy` is currently iterating.
    ///
    /// Those instructions bind `i` (and `l`/`r`, `i`/`j`) to a **slot**
    /// ordinal, but a bare integer passed to `area`/`depth`/`get_data` used to
    /// mean a **shape** index. Those agree only when a group's slots are
    /// `[[0],[1],[2]...]` - true for `main`, false for anything a `Filter`
    /// derived - so `area(i)` silently measured an unrelated shape. Knowing
    /// which group is being iterated is what lets a bare integer resolve to
    /// the right slot.
    pub current_group: Arc<Mutex<Option<String>>>,
}

/// A `Define`d function: its parameter names and its body.
#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub params: Vec<String>,
    pub instructions: Vec<crate::Instruction>,
}

/// A failed `Assert`, collected rather than thrown in production mode.
#[derive(Debug, Clone)]
pub struct Warning {
    /// Order of failure across the whole run.
    pub sequence: usize,
    /// The assert's stable `id`, so repeats can be counted and grouped.
    pub id: String,
    /// Already-expanded text. `Assert.message` is a template, so the program
    /// embeds its own context ("part 3 of sheet 1") rather than gel guessing
    /// what context to capture.
    pub message: String,
    /// The group being iterated when it failed, if any.
    pub group: Option<String>,
}

/// One operator-facing label: what it says, how big, and where it goes.
///
/// Position is in the same coordinate space as the shapes.
#[derive(Debug, Clone)]
pub struct Annotation {
    pub text: String,
    pub font: String,
    pub size: f64,
    /// CSS colour, e.g. `"black"` or `"rgb(200,0,0)"`. Labels carry their own
    /// colour so a warning can be red while a header stays black.
    pub color: String,
    pub x: f64,
    pub y: f64,
}

fn get_polygons(
    shapes: &Arc<Mutex<Vec<Polygon>>>,
    groups: &Arc<Mutex<HashMap<String, Vec<Vec<usize>>>>>,
    current: &Arc<Mutex<Option<String>>>,
    args: &[JsValue],
) -> Vec<Polygon> {
    let shapes = shapes.lock().unwrap();
    let groups = groups.lock().unwrap();
    let current = current.lock().unwrap();

    let mut iter = args.iter();
    match (iter.next(), iter.next(), iter.next()) {
        // Bare integer: a slot of the group being iterated, or a shape index
        // outside an iteration. See `bare_index_targets`.
        (Some(JsValue::Integer(index)), _, _) => {
            bare_index_targets(&groups, &current, *index as usize)
                .iter()
                .filter_map(|&i| shapes.get(i))
                .cloned()
                .collect()
        }
        (
            Some(JsValue::String(name)),
            Some(JsValue::Integer(index1)),
            Some(JsValue::Integer(index2)),
        ) => vec![
            shapes[groups[&name.to_std_string_lossy()][*index1 as usize][*index2 as usize]].clone(),
        ],
        (Some(JsValue::String(name)), Some(JsValue::Integer(index)), None) => {
            if let Some(group) = groups.get(&name.to_std_string_lossy()) {
                let mut polygons = Vec::new();
                for index in &group[*index as usize] {
                    polygons.push(shapes[*index].clone());
                }
                polygons
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

fn get_points(polygons: &[Polygon]) -> Vec<Point> {
    polygons
        .into_iter()
        .map(|polygon| polygon.exterior().points())
        .flatten()
        .collect::<Vec<Point>>()
}

/// A slot's contents ARE the shape indices belonging to it - this
/// is also the "free union" from the design discussion: whatever
/// polygons end up sharing a slot are already treated as one set by
/// every consumer (area sums them, offset/boolean ops below combine
/// them into one MultiPolygon), no separate union call needed
/// unless the caller actually needs overlap-correct merged geometry
/// (that's what the real `union` op below is for).
pub fn resolve_indices(groups: &HashMap<String, Vec<Vec<usize>>>, label: &str, index: usize) -> Vec<usize> {
    groups.get(label).and_then(|g| g.get(index)).cloned().unwrap_or_default()
}

pub fn multipolygon_from_indices(shapes: &[Polygon], indices: &[usize]) -> MultiPolygon {
    MultiPolygon::new(indices.iter().map(|&i| shapes[i].clone()).collect())
}

/// Writes `polygons` as new shapes appended to the (append-only,
/// never-mutated-in-place) shape store, and points `(out_label,
/// out_index)` at exactly those new indices - touching only that
/// one slot, every other slot already under `out_label` is left
/// alone. Returns how many shapes were written (0 if the op
/// produced nothing, e.g. a shape collapsed entirely).
pub fn write_result_slot(
    shapes: &mut Vec<Polygon>,
    depths: &mut Vec<usize>,
    groups: &mut HashMap<String, Vec<Vec<usize>>>,
    out_label: &str,
    out_index: usize,
    polygons: Vec<Polygon>,
) -> usize {
    let start = shapes.len();
    let count = polygons.len();
    shapes.extend(polygons);
    // `depths` is indexed directly (not via `.get()`) by `depth(i)`,
    // `stats`, and `list_shapes` - it must stay exactly as long as
    // `shapes` or those panic on any derived shape's index. Derived
    // shapes aren't part of the original document's containment
    // tree, so 0 is the honest depth to give them (not a guess at
    // a "real" nesting level that doesn't apply here).
    depths.resize(shapes.len(), 0);
    let new_indices: Vec<usize> = (start..start + count).collect();
    let entry = groups.entry(out_label.to_string()).or_default();
    if entry.len() <= out_index {
        entry.resize(out_index + 1, Vec::new());
    }
    entry[out_index] = new_indices;
    count
}

/// Which shapes a bare integer argument refers to.
///
/// Inside a `Filter`/`Sort`/`GroupBy` it is a slot ordinal of the group being
/// iterated; outside one there is no group in scope and it is a shape index.
/// A group with one shape per slot gives the same answer either way, which is
/// why this went unnoticed for so long.
pub fn bare_index_targets(
    groups: &HashMap<String, Vec<Vec<usize>>>,
    current: &Option<String>,
    index: usize,
) -> Vec<usize> {
    match current {
        Some(name) => groups
            .get(name)
            .and_then(|slots| slots.get(index))
            .cloned()
            .unwrap_or_default(),
        None => vec![index],
    }
}

/// Runs `body` with `group` marked as the one being iterated, restoring
/// whatever was current before. Restoring (rather than clearing) matters
/// because a `Filter` can sit inside a `LoopOver` inside another `Filter`.
pub fn with_current_group<T>(
    current: &Arc<Mutex<Option<String>>>,
    group: &str,
    body: impl FnOnce() -> T,
) -> T {
    let previous = current.lock().unwrap().replace(group.to_string());
    let result = body();
    *current.lock().unwrap() = previous;
    result
}

/// gel's own colour string: `rgba(r,g,b,a)` with a 0-255 alpha, which is
/// what `fill_color`/`stroke_color` return and what `color_distance` parses.
/// Deliberately not CSS (CSS alpha is 0-1) - `Emit` converts on the way out,
/// so existing programs comparing against this format keep working.
pub fn rgba_to_js_string(rgba: Option<(u8, u8, u8, u8)>) -> String {
    match rgba {
        Some((r, g, b, a)) => format!("rgba({r},{g},{b},{a})"),
        None => "none".into(),
    }
}

/// Reads a string-valued key out of the shape store, for the JS globals.
fn string_key(
    store: &Arc<Mutex<Vec<serde_json::Map<String, serde_json::Value>>>>,
    groups: &Arc<Mutex<HashMap<String, Vec<Vec<usize>>>>>,
    current: &Arc<Mutex<Option<String>>>,
    index: Option<&JsValue>,
    key: &str,
    default: &str,
) -> String {
    let Some(JsValue::Integer(index)) = index else {
        return default.to_string();
    };
    let targets = {
        let groups = groups.lock().unwrap();
        let current = current.lock().unwrap();
        bare_index_targets(&groups, &current, *index as usize)
    };
    let store = store.lock().unwrap();
    // A slot's shapes share their paint in practice, so the first one answers
    // for the slot.
    targets
        .first()
        .and_then(|&i| store.get(i))
        .and_then(|m| m.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or(default)
        .to_string()
}

impl Data {
    /// Grows the per-shape style vectors to match `shapes`.
    ///
    /// Instructions append derived shapes via [`write_result_slot`], which
    /// touches only `shapes` and `depths`. Without this the style vectors go
    /// short, and because `Emit` reads them with `.get(i)` the result is not a
    /// panic but silence: every derived shape emits unstyled. Called after
    /// each instruction, so a new instruction cannot reintroduce the bug.
    ///
    /// Derived geometry starts with an empty map; `SetData` is how a program
    /// says otherwise.
    pub fn sync_style_lengths(&self) {
        let len = self.shapes.lock().unwrap().len();
        self.shape_data
            .lock()
            .unwrap()
            .resize(len, serde_json::Map::new());
    }
}

/// Precision-scale factor for geo_clipper's internal integer
/// arithmetic (it converts f64 coords to i64 via this factor) -
/// same value `kahm_cam` already uses in production for this same
/// inch-scale sign/CAM geometry.
pub const CLIPPER_FACTOR: f64 = 32768.0; // 2^15

/// Builds the `geo_clipper` join type for an `Offset`.
///
/// For a round join, `value` is the arc tolerance **in drawing units** - how
/// far the emitted polyline may deviate from the true arc. That has to be
/// scaled here: `geo-clipper` multiplies the geometry by `CLIPPER_FACTOR`
/// and the offset distance by `CLIPPER_FACTOR`, but passes `JoinType`'s own
/// payload through untouched (`execute_offset_operation`). Measured against
/// integer-scaled input, an unscaled value comes out 32768x finer than the
/// number in the program.
///
/// For a miter join, `value` is the miter limit - a dimensionless ratio, so
/// no scaling applies. `square` ignores it.
pub fn join_type_from(name: &str, value: f64) -> geo_clipper::JoinType {
    match name {
        "miter" => geo_clipper::JoinType::Miter(value),
        "square" => geo_clipper::JoinType::Square,
        _ => geo_clipper::JoinType::Round(value * CLIPPER_FACTOR),
    }
}


/// The geometry primitives, as plain functions.
///
/// These were reachable only as JS globals, which meant the only way to call
/// one was to evaluate a `Filter` predicate over a one-element group purely
/// for its side effect (the `run_once` hack in the layout pipeline). Both the
/// JS bindings below and the `Offset`/`Union`/`Intersect`/`Difference`/
/// `Flatten`/`Copy` instructions now call these, so there is one
/// implementation and the two entry points cannot drift.
///
/// Every operand is a `(label, index)` slot; results are appended to the
/// append-only shape store and the target slot is pointed at them. Each
/// returns how many shapes were written.
pub fn offset_slot(
    shapes: &Arc<Mutex<Vec<Polygon>>>,
    depths: &Arc<Mutex<Vec<usize>>>,
    groups: &Arc<Mutex<HashMap<String, Vec<Vec<usize>>>>>,
    in_label: &str,
    in_index: usize,
    out_label: &str,
    out_index: usize,
    distance: f64,
    join_name: &str,
    join_value: f64,
) -> usize {
    use geo_clipper::Clipper;
    let mut shapes = shapes.lock().unwrap();
    let mut depths = depths.lock().unwrap();
    let mut groups = groups.lock().unwrap();
    let indices = resolve_indices(&groups, in_label, in_index);
    let source = multipolygon_from_indices(&shapes, &indices);
    let result = source.offset(
        distance,
        join_type_from(join_name, join_value),
        geo_clipper::EndType::ClosedPolygon,
        CLIPPER_FACTOR,
    );
    write_result_slot(&mut shapes, &mut depths, &mut groups, out_label, out_index, result.0)
}

pub fn boolean_slot(
    shapes: &Arc<Mutex<Vec<Polygon>>>,
    depths: &Arc<Mutex<Vec<usize>>>,
    groups: &Arc<Mutex<HashMap<String, Vec<Vec<usize>>>>>,
    a_label: &str,
    a_index: usize,
    b_label: &str,
    b_index: usize,
    out_label: &str,
    out_index: usize,
    op: fn(&MultiPolygon, &MultiPolygon, f64) -> MultiPolygon,
) -> usize {
    let mut shapes = shapes.lock().unwrap();
    let mut depths = depths.lock().unwrap();
    let mut groups = groups.lock().unwrap();
    let a = multipolygon_from_indices(&shapes, &resolve_indices(&groups, a_label, a_index));
    let b = multipolygon_from_indices(&shapes, &resolve_indices(&groups, b_label, b_index));
    let result = op(&a, &b, CLIPPER_FACTOR);
    write_result_slot(&mut shapes, &mut depths, &mut groups, out_label, out_index, result.0)
}

/// Collapses every slot of `label` into the single slot `(out_label,
/// out_index)`. No geometry is created - it only re-points membership - which
/// is what the boolean/offset ops need, since each resolves exactly one slot
/// per operand.
pub fn flatten_slot(
    groups: &Arc<Mutex<HashMap<String, Vec<Vec<usize>>>>>,
    label: &str,
    out_label: &str,
    out_index: usize,
) -> usize {
    let mut groups = groups.lock().unwrap();
    let all: Vec<usize> = groups
        .get(label)
        .map(|g| g.iter().flatten().copied().collect())
        .unwrap_or_default();
    let count = all.len();
    let entry = groups.entry(out_label.to_string()).or_default();
    if entry.len() <= out_index {
        entry.resize(out_index + 1, Vec::new());
    }
    entry[out_index] = all;
    count
}

/// Duplicates the polygons in a slot as genuinely new shapes, rather than
/// pointing a second name at the same indices. The inlaid procedure needs
/// this: it keeps an unaltered copy of the text while the original is offset.
pub fn copy_slot(
    shapes: &Arc<Mutex<Vec<Polygon>>>,
    depths: &Arc<Mutex<Vec<usize>>>,
    groups: &Arc<Mutex<HashMap<String, Vec<Vec<usize>>>>>,
    in_label: &str,
    in_index: usize,
    out_label: &str,
    out_index: usize,
) -> usize {
    let mut shapes = shapes.lock().unwrap();
    let mut depths = depths.lock().unwrap();
    let mut groups = groups.lock().unwrap();
    let indices = resolve_indices(&groups, in_label, in_index);
    let copies: Vec<Polygon> = indices.iter().map(|&i| shapes[i].clone()).collect();
    write_result_slot(&mut shapes, &mut depths, &mut groups, out_label, out_index, copies)
}

impl From<Vec<Polygon>> for Data {
    fn from(value: Vec<Polygon>) -> Self {
        Self::from_respect_indexes(value).0
    }
}

impl From<Box<std::path::Path>> for Data {
    fn from(value: Box<std::path::Path>) -> Self {
        let lines = depth_tree::import_svg(&(*value), 0.0001).unwrap();

        let polygons: Vec<Polygon> = lines
            .into_iter()
            .map(|line| Polygon::new(line, Vec::new()))
            .collect();

        polygons.into()
    }
}

impl From<(Box<std::path::Path>, f64)> for Data {
    fn from(value: (Box<std::path::Path>, f64)) -> Self {
        let lines =
            MultiLineString::new(depth_tree::import_svg(&(*value.0), value.1 as f32).unwrap());
        eprintln!("DONE THIS");
        let mut lines = lines.simplify(value.1);
        eprintln!("Simplified");

        let (min_x, min_y) = lines.bounding_rect().unwrap().min().x_y();
        lines.translate_mut(-min_x, -min_y);
        // lines.scale_mut(0.01);
        let lines = lines.0;

        let polygons: Vec<Polygon> = lines
            .into_iter()
            .map(|line| Polygon::new(line, Vec::new()))
            .collect();

        polygons.into()
    }
}

impl Data {
    pub fn query<T: Query>(&mut self, queries: Vec<T>) -> Result<(), String> {
        let mut i = 0;
        let n = queries.len();
        for mut query in queries {
            eprintln!("QUERY: {} out of {}", i, n);
            i += 1;
            query.query(self)?;
        }

        Ok(())
    }

    pub fn from_respect_indexes(value: Vec<Polygon>) -> (Self, Vec<usize>) {
        let value: Vec<(usize, Polygon)> = value.into_iter().enumerate().collect();
        let len = value.len();
        eprintln!("Len: {}", len);
        let tree: Tree<(usize, Polygon)> = Tree::from_polygon_id(value);
        eprintln!("Built Tree");

        let mut shapes = Vec::with_capacity(len);
        let mut depths = Vec::with_capacity(len);
        let mut indexes = Vec::with_capacity(len);

        for (depth, polygon) in tree.iter() {
            shapes.push(polygon.1.clone());
            indexes.push(polygon.0);
            depths.push(depth);
        }

        let shapes = Arc::new(Mutex::new(shapes));
        let depths = Arc::new(Mutex::new(depths));
        let current_group: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let current_group_field = current_group.clone();
        let groups = Arc::new(Mutex::new(
            vec![("main".into(), (0..len).map(|x| vec![x]).collect())]
                .into_iter()
                .collect::<HashMap<String, Vec<Vec<usize>>>>(),
        ));

        let mut context = Context::default();
        unsafe {
            {
                let depths = depths.clone();
                let groups_for_depth = groups.clone();
                let current_for_depth = current_group.clone();
                context.register_global_callable(
                    "depth".into(),
                    0,
                    NativeFunction::from_closure(
                        move |this: &JsValue, args: &[JsValue], context: &mut Context| {
                            let depths = depths.lock().unwrap();
                            match args.first() {
                                Some(JsValue::Integer(index)) => {
                                    let targets = {
                                        let groups = groups_for_depth.lock().unwrap();
                                        let current = current_for_depth.lock().unwrap();
                                        bare_index_targets(&groups, &current, *index as usize)
                                    };
                                    let depth = targets
                                        .first()
                                        .and_then(|&i| depths.get(i))
                                        .copied()
                                        .unwrap_or(0);
                                    JsResult::Ok(JsValue::new(depth))
                                }
                                _ => JsResult::Ok(JsValue::new(0.0)),
                            }
                        },
                    ),
                );
            }

            {
                let shapes = shapes.clone();
                let groups = groups.clone();
                let current_for_area = current_group.clone();
                context.register_global_callable(
                    "area".into(),
                    0,
                    NativeFunction::from_closure(
                        move |this: &JsValue, args: &[JsValue], context: &mut Context| {
                            let shapes = shapes.lock().unwrap();
                            let groups = groups.lock().unwrap();
                            let mut iter = args.iter();
                            match (iter.next(), iter.next(), iter.next()) {
                                (Some(JsValue::Integer(index)), _, _) => {
                                    let current = current_for_area.lock().unwrap();
                                    let area = bare_index_targets(
                                        &groups,
                                        &current,
                                        *index as usize,
                                    )
                                    .iter()
                                    .filter_map(|&i| shapes.get(i))
                                    .map(|s| s.unsigned_area())
                                    .sum::<f64>();
                                    JsResult::Ok(JsValue::new(area))
                                }
                                (
                                    Some(JsValue::String(name)),
                                    Some(JsValue::Integer(index1)),
                                    Some(JsValue::Integer(index2)),
                                ) => JsResult::Ok(JsValue::new(
                                    shapes[groups[&name.to_std_string_lossy()][*index1 as usize]
                                        [*index2 as usize]]
                                        .unsigned_area(),
                                )),
                                (
                                    Some(JsValue::String(name)),
                                    Some(JsValue::Integer(index)),
                                    None,
                                ) => {
                                    if let Some(group) = groups.get(&name.to_std_string_lossy()) {
                                        let mut area = 0.0;
                                        for index in &group[*index as usize] {
                                            area += shapes[*index].unsigned_area();
                                        }

                                        JsResult::Ok(JsValue::new(area))
                                    } else {
                                        JsResult::Ok(JsValue::new(0.0))
                                    }
                                }
                                _ => JsResult::Ok(JsValue::new(0.0)),
                            }
                        },
                    ),
                );
            }

            {
                let groups = groups.clone();
                context.register_global_callable(
                    "group_index".into(),
                    0,
                    NativeFunction::from_closure(
                        move |this: &JsValue, args: &[JsValue], context: &mut Context| {
                            let groups = groups.lock().unwrap();
                            let mut iter = args.iter();
                            match (iter.next(), iter.next(), iter.next()) {
                                (
                                    Some(JsValue::String(name)),
                                    Some(JsValue::Integer(index1)),
                                    Some(JsValue::Integer(index2)),
                                ) => {
                                    let name: String = name.to_std_string_lossy();
                                    if let Some(group) = groups.get(&name) {
                                        if *index1 < 0 || *index1 as usize >= group.len() {
                                            JsResult::Err(JsError::from_opaque(
                                                js_string!("Index out of bounds").into(),
                                            ))
                                        } else if *index2 < 0
                                            || *index2 as usize >= group[*index1 as usize].len()
                                        {
                                            JsResult::Err(JsError::from_opaque(
                                                js_string!("Index out of bounds").into(),
                                            ))
                                        } else {
                                            JsResult::Ok(JsValue::new(
                                                group[*index1 as usize][*index2 as usize],
                                            ))
                                        }
                                    } else {
                                        JsResult::Err(JsError::from_opaque(
                                            js_string!("Name not found in groups.").into(),
                                        ))
                                    }
                                }
                                _ => JsResult::Ok(JsValue::new(0.0)),
                            }
                        },
                    ),
                );
            }

            {
                let shapes = shapes.clone();
                let groups = groups.clone();
                let current_for_frame = current_group.clone();
                context.register_global_callable(
                    "frame".into(),
                    0,
                    NativeFunction::from_closure(
                        move |this: &JsValue, args: &[JsValue], context: &mut Context| {
                            let shapes = shapes.lock().unwrap();
                            let groups = groups.lock().unwrap();
                            let mut iter = args.iter();
                            match (iter.next(), iter.next(), iter.next()) {
                                (Some(JsValue::Integer(index)), _, _) => {
                                    let targets = {
                                        let current = current_for_frame.lock().unwrap();
                                        bare_index_targets(&groups, &current, *index as usize)
                                    };
                                    let slot = MultiPolygon::new(
                                        targets
                                            .iter()
                                            .filter_map(|&i| shapes.get(i))
                                            .cloned()
                                            .collect(),
                                    );
                                    if let Some(bounding_rect) = slot.bounding_rect() {
                                        let object = ObjectInitializer::new(context)
                                            .property(
                                                js_string!("height"),
                                                bounding_rect.height(),
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("width"),
                                                bounding_rect.width(),
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("min_x"),
                                                bounding_rect.min().x,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("min_y"),
                                                bounding_rect.min().y,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("max_x"),
                                                bounding_rect.max().x,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("max_y"),
                                                bounding_rect.max().y,
                                                Attribute::all(),
                                            )
                                            .build();
                                        JsResult::Ok(JsValue::new(object))
                                    } else {
                                        JsResult::Ok(JsValue::new(0.0))
                                    }
                                }
                                (
                                    Some(JsValue::String(name)),
                                    Some(JsValue::Integer(index1)),
                                    Some(JsValue::Integer(index2)),
                                ) => {
                                    if let Some(bounding_rect) = shapes[groups
                                        [&name.to_std_string_lossy()]
                                        [*index1 as usize]
                                        [*index2 as usize]]
                                        .bounding_rect()
                                    {
                                        let object = ObjectInitializer::new(context)
                                            .property(
                                                js_string!("height"),
                                                bounding_rect.height(),
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("width"),
                                                bounding_rect.width(),
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("min_x"),
                                                bounding_rect.min().x,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("min_y"),
                                                bounding_rect.min().y,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("max_x"),
                                                bounding_rect.max().x,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("max_y"),
                                                bounding_rect.max().y,
                                                Attribute::all(),
                                            )
                                            .build();
                                        JsResult::Ok(JsValue::new(object))
                                    } else {
                                        JsResult::Ok(JsValue::new(0.0))
                                    }
                                }
                                (
                                    Some(JsValue::String(name)),
                                    Some(JsValue::Integer(index)),
                                    None,
                                ) => {
                                    if let Some(bounding_rect) = MultiPolygon::new(
                                        groups[&name.to_std_string_lossy()][*index as usize]
                                            .iter()
                                            .map(|index| shapes[*index].clone())
                                            .collect::<Vec<Polygon>>(),
                                    )
                                    .bounding_rect()
                                    {
                                        let object = ObjectInitializer::new(context)
                                            .property(
                                                js_string!("height"),
                                                bounding_rect.height(),
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("width"),
                                                bounding_rect.width(),
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("min_x"),
                                                bounding_rect.min().x,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("min_y"),
                                                bounding_rect.min().y,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("max_x"),
                                                bounding_rect.max().x,
                                                Attribute::all(),
                                            )
                                            .property(
                                                js_string!("max_y"),
                                                bounding_rect.max().y,
                                                Attribute::all(),
                                            )
                                            .build();
                                        JsResult::Ok(JsValue::new(object))
                                    } else {
                                        JsResult::Ok(JsValue::new(0.0))
                                    }
                                }
                                _ => JsResult::Ok(JsValue::new(0.0)),
                            }
                        },
                    ),
                );
            }

            {
                let shapes = shapes.clone();
                let groups = groups.clone();
                let current_for_len = current_group.clone();
                context.register_global_callable(
                    "len".into(),
                    0,
                    NativeFunction::from_closure(
                        move |this: &JsValue, args: &[JsValue], context: &mut Context| {
                            let shapes = shapes.lock().unwrap();
                            let groups = groups.lock().unwrap();
                            let mut iter = args.iter();
                            match (iter.next(), iter.next(), iter.next()) {
                                (Some(JsValue::Integer(index)), _, _) => {
                                    let current = current_for_len.lock().unwrap();
                                    let rings = bare_index_targets(
                                        &groups,
                                        &current,
                                        *index as usize,
                                    )
                                    .iter()
                                    .filter_map(|&i| shapes.get(i))
                                    .map(|s| s.rings().count())
                                    .sum::<usize>();
                                    JsResult::Ok(JsValue::new(rings))
                                }
                                (
                                    Some(JsValue::String(name)),
                                    Some(JsValue::Integer(index1)),
                                    Some(JsValue::Integer(index2)),
                                ) => JsResult::Ok(JsValue::new(
                                    shapes[groups[&name.to_std_string_lossy()][*index1 as usize]
                                        [*index2 as usize]]
                                        .rings()
                                        .count(),
                                )),
                                (
                                    Some(JsValue::String(name)),
                                    Some(JsValue::Integer(index)),
                                    None,
                                ) => JsResult::Ok(JsValue::new(
                                    groups[&name.to_std_string_lossy()][*index as usize].len(),
                                )),
                                (Some(JsValue::String(name)), None, None) => JsResult::Ok(
                                    JsValue::new(groups[&name.to_std_string_lossy()].len()),
                                ),
                                _ => JsResult::Ok(JsValue::new(0.0)),
                            }
                        },
                    ),
                );
            }

            {
                let groups = groups.clone();
                let _ = context.register_global_callable(
                    "shape_count".into(),
                    0,
                    NativeFunction::from_closure(
                        move |_this: &JsValue, args: &[JsValue], _context: &mut Context| {
                            // `len(g)` counts SLOTS and `len(g, i)` counts the shapes in
                            // one slot, so totalling a group used to need a JS loop.
                            // That gap let a real assert pass when it should not have:
                            // `SetOp append` had added an empty slot for a part that
                            // vanished, so the slot counts matched while a shape was
                            // missing.
                            let Some(JsValue::String(name)) = args.first() else {
                                return JsResult::Err(JsError::from_opaque(
                                    js_string!("shape_count(group) takes one group name").into(),
                                ));
                            };
                            let name = name.to_std_string_lossy();
                            let groups = groups.lock().unwrap();
                            match groups.get(&name) {
                                Some(slots) => JsResult::Ok(JsValue::new(
                                    slots.iter().map(|slot| slot.len()).sum::<usize>(),
                                )),
                                // An unknown group is a program bug, not a count of
                                // zero - returning 0 would make a guard silently pass.
                                None => JsResult::Err(JsError::from_opaque(
                                    js_string!(format!("shape_count: unknown group '{name}'")).into(),
                                )),
                            }
                        },
                    ),
                );
            }

            {
                let shapes = shapes.clone();
                let groups = groups.clone();
                let current_for_center = current_group.clone();
                context.register_global_callable(
                    "center".into(),
                    0,
                    NativeFunction::from_closure(
                        move |this: &JsValue, args: &[JsValue], context: &mut Context| {
                            let polygons = get_polygons(&shapes, &groups, &current_for_center, args);
                            let points = get_points(&polygons);
                            let total = points.iter().fold(Point::new(0.0, 0.0), |mut acc, &x| {
                                acc += x;
                                acc
                            });
                            let total = total / (points.len() as f64);

                            let object = ObjectInitializer::new(context)
                                .property(js_string!("x"), total.x(), Attribute::all())
                                .property(js_string!("y"), total.y(), Attribute::all())
                                .build();
                            JsResult::Ok(JsValue::new(object))
                        },
                    ),
                );
            }

            {
                let shapes = shapes.clone();
                let groups = groups.clone();
                let current_for_circle = current_group.clone();
                context.register_global_callable(
                    "circle_metrics".into(),
                    0,
                    NativeFunction::from_closure(
                        move |this: &JsValue, args: &[JsValue], context: &mut Context| {
                            let polygons = get_polygons(&shapes, &groups, &current_for_circle, &args);
                            let points = get_points(&polygons);
                            let total = points.iter().fold(Point::new(0.0, 0.0), |mut acc, &x| {
                                acc += x;
                                acc
                            });
                            let total = total / (points.len() as f64);

                            let mut distances = Vec::new();
                            let mut total_d = 0.;
                            for point in &points {
                                use geo::{Distance, Euclidean};

                                let d = Euclidean.distance(*point, total);
                                total_d += d;
                                distances.push(d);
                            }

                            let average_d = total_d / points.len() as f64;
                            let mut variance = 0.0;
                            for d in &distances {
                                variance += (d - average_d).powi(2);
                            }

                            let object = ObjectInitializer::new(context)
                                .property(js_string!("variance"), variance, Attribute::all())
                                .property(
                                    js_string!("circle"),
                                    1.0 - variance / average_d,
                                    Attribute::all(),
                                )
                                .build();
                            JsResult::Ok(JsValue::new(object))
                        },
                    ),
                );
            }

            {
                let shapes = shapes.clone();
                let groups = groups.clone();
                let current_for_distance = current_group.clone();
                context.register_global_callable(
                    "distance".into(),
                    0,
                    NativeFunction::from_closure(
                        move |_this: &JsValue, args: &[JsValue], _context: &mut Context| {
                            let mut index = 0;
                            for arg in args {
                                // skip the first one
                                if index == 0 {
                                    index += 1;
                                    continue;
                                }

                                if let JsValue::String(_) = arg {
                                    break;
                                }
                                index += 1;
                            }

                            let (first, second) = args.split_at(index);

                            let first = get_polygons(&shapes, &groups, &current_for_distance, &first);
                            let second =
                                get_polygons(&shapes, &groups, &current_for_distance, &second);

                            let first = MultiPolygon::from(first);
                            let second = MultiPolygon::from(second);

                            use geo::{Distance, Euclidean};
                            let distance = Euclidean.distance(&first, &second);

                            JsResult::Ok(JsValue::new(distance))
                        },
                    ),
                );
            }
        }

        let shape_data: Arc<Mutex<Vec<serde_json::Map<String, serde_json::Value>>>> =
            Arc::new(Mutex::new(Vec::new()));

        // Shared helpers for offset/union/intersect/difference: every
        // operand is (label, index=0) - "label" a group name, "index" which
        // slot of that group (a slot is already a Vec<usize> of shape
        // indices, so it doubles as a free "union" for measurement/
        // addressing purposes - see the operand-resolution note on
        // `resolve_indices`). Parsing is a streaming peek, not a fixed
        // arg count, matching how `area`/`group_index` already parse their
        // args: consume a String as the label, then peek - if the next arg
        // is an Integer, consume it as the index; otherwise the index
        // defaults to 0 and nothing is consumed, so the next String starts
        // the following operand. This lets callers omit an index whenever
        // they mean the default, exactly like `offset("input", "temp", d)`
        // vs `intersect("input", i, "bad_area", "temp")`.
        fn take_label(iter: &mut std::iter::Peekable<std::slice::Iter<JsValue>>) -> Option<String> {
            match iter.next() {
                Some(JsValue::String(s)) => Some(s.to_std_string_lossy()),
                _ => None,
            }
        }
        fn take_optional_index(iter: &mut std::iter::Peekable<std::slice::Iter<JsValue>>) -> usize {
            match iter.peek() {
                Some(JsValue::Integer(n)) => {
                    let n = *n;
                    iter.next();
                    n as usize
                }
                _ => 0,
            }
        }
        fn take_number(iter: &mut std::iter::Peekable<std::slice::Iter<JsValue>>) -> Option<f64> {
            match iter.next() {
                Some(JsValue::Integer(n)) => Some(*n as f64),
                Some(JsValue::Rational(n)) => Some(*n),
                _ => None,
            }
        }
        fn take_optional_string(iter: &mut std::iter::Peekable<std::slice::Iter<JsValue>>, default: &str) -> String {
            match iter.peek() {
                Some(JsValue::String(s)) => {
                    let s = s.to_std_string_lossy();
                    iter.next();
                    s
                }
                _ => default.to_string(),
            }
        }
        fn take_optional_number(iter: &mut std::iter::Peekable<std::slice::Iter<JsValue>>, default: f64) -> f64 {
            match iter.peek() {
                Some(JsValue::Integer(_)) | Some(JsValue::Rational(_)) => take_number(iter).unwrap_or(default),
                _ => default,
            }
        }


        /// Parses `"rgb(r,g,b)"` or this crate's own `"rgba(r,g,b,a)"`
        /// string format (as returned by `fill_color`/`stroke_color`) back
        /// into components, for `color_distance`. Alpha is `None` when the
        /// input was plain `"rgb(...)"` (no alpha given at all, not "alpha
        /// 255") - `color_distance` only factors alpha into the distance
        /// when BOTH sides actually specified one. Returns `None` for
        /// `"none"` or anything else that isn't one of those two formats -
        /// deliberately not a general CSS color parser.
        fn parse_rgb_string(s: &str) -> Option<(f64, f64, f64, Option<f64>)> {
            let (inner, has_alpha) = if let Some(inner) = s.strip_prefix("rgba(") {
                (inner, true)
            } else {
                (s.strip_prefix("rgb(")?, false)
            };
            let inner = inner.strip_suffix(")")?;
            let mut parts = inner.split(',').map(|p| p.trim().parse::<f64>());
            let r = parts.next()?.ok()?;
            let g = parts.next()?.ok()?;
            let b = parts.next()?.ok()?;
            let a = if has_alpha { Some(parts.next()?.ok()?) } else { None };
            Some((r, g, b, a))
        }

        unsafe {
            {
                let shape_data = shape_data.clone();
                let groups_for_fill = groups.clone();
                let current_for_fill = current_group.clone();
                let _ = context.register_global_callable(
                    "fill_color".into(),
                    0,
                    NativeFunction::from_closure(
                        move |_this: &JsValue, args: &[JsValue], _context: &mut Context| {
                            JsResult::Ok(JsValue::new(js_string!(string_key(
                                &shape_data,
                                &groups_for_fill,
                                &current_for_fill,
                                args.first(),
                                "fill",
                                "none"
                            ))))
                        },
                    ),
                );
            }
            {
                let shape_data = shape_data.clone();
                let groups_for_stroke = groups.clone();
                let current_for_stroke = current_group.clone();
                let _ = context.register_global_callable(
                    "stroke_color".into(),
                    0,
                    NativeFunction::from_closure(
                        move |_this: &JsValue, args: &[JsValue], _context: &mut Context| {
                            JsResult::Ok(JsValue::new(js_string!(string_key(
                                &shape_data,
                                &groups_for_stroke,
                                &current_for_stroke,
                                args.first(),
                                "stroke",
                                "none"
                            ))))
                        },
                    ),
                );
            }
            {
                let shape_data = shape_data.clone();
                let groups_for_visible = groups.clone();
                let current_for_visible = current_group.clone();
                let _ = context.register_global_callable(
                    "is_visible".into(),
                    0,
                    NativeFunction::from_closure(
                        move |_this: &JsValue, args: &[JsValue], _context: &mut Context| {
                            let visible = match args.first() {
                                Some(JsValue::Integer(index)) => {
                                    let targets = {
                                        let groups = groups_for_visible.lock().unwrap();
                                        let current = current_for_visible.lock().unwrap();
                                        bare_index_targets(&groups, &current, *index as usize)
                                    };
                                    let store = shape_data.lock().unwrap();
                                    targets
                                        .first()
                                        .and_then(|&i| store.get(i))
                                        .and_then(|m| m.get("visible"))
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(true)
                                }
                                _ => true,
                            };
                            JsResult::Ok(JsValue::new(visible))
                        },
                    ),
                );
            }
            {
                // The generic reader. Mirrors `area`'s argument forms, which
                // matters because `Filter` binds `i` to a *slot* index while
                // a bare integer here means a *shape* index - they only
                // coincide when a group has one shape per slot:
                //
                //   get_data(shape, "key")
                //   get_data("group", slot, "key")         - first shape in the slot
                //   get_data("group", slot, pos, "key")    - one shape in the slot
                let shape_data = shape_data.clone();
                let groups_for_data = groups.clone();
                let current_for_data = current_group.clone();
                let _ = context.register_global_callable(
                    "get_data".into(),
                    0,
                    NativeFunction::from_closure(
                        move |_this: &JsValue, args: &[JsValue], context: &mut Context| {
                            let store = shape_data.lock().unwrap();
                            let groups = groups_for_data.lock().unwrap();

                            let slot_of = |name: &JsValue, slot: &JsValue| -> Option<Vec<usize>> {
                                let (JsValue::String(name), JsValue::Integer(slot)) = (name, slot)
                                else {
                                    return None;
                                };
                                groups
                                    .get(&name.to_std_string_lossy())
                                    .and_then(|g| g.get(*slot as usize))
                                    .cloned()
                            };
                            let read = |index: usize, key: &JsValue| -> Option<serde_json::Value> {
                                let JsValue::String(key) = key else { return None };
                                store
                                    .get(index)
                                    .and_then(|m| m.get(&key.to_std_string_lossy()))
                                    .cloned()
                            };

                            let value = match args {
                                [JsValue::Integer(index), key] => {
                                    let current = current_for_data.lock().unwrap();
                                    bare_index_targets(&groups, &current, *index as usize)
                                        .first()
                                        .and_then(|&i| read(i, key))
                                }
                                [name, slot, key] => slot_of(name, slot)
                                    .and_then(|s| s.first().copied())
                                    .and_then(|index| read(index, key)),
                                [name, slot, JsValue::Integer(pos), key] => slot_of(name, slot)
                                    .and_then(|s| s.get(*pos as usize).copied())
                                    .and_then(|index| read(index, key)),
                                _ => None,
                            };

                            match value {
                                Some(value) => JsValue::from_json(&value, context),
                                // Absent reads as undefined, so
                                // `get_data(...) == undefined` is the
                                // idiomatic "was this ever set" check.
                                None => JsResult::Ok(JsValue::undefined()),
                            }
                        },
                    ),
                );
            }
            {
                let _ = context.register_global_callable(
                    "color_distance".into(),
                    0,
                    NativeFunction::from_closure(
                        move |_this: &JsValue, args: &[JsValue], _context: &mut Context| {
                            let mut iter = args.iter();
                            match (iter.next(), iter.next()) {
                                (Some(JsValue::String(a)), Some(JsValue::String(b))) => {
                                    match (
                                        parse_rgb_string(&a.to_std_string_lossy()),
                                        parse_rgb_string(&b.to_std_string_lossy()),
                                    ) {
                                        (Some((r1, g1, b1, a1)), Some((r2, g2, b2, a2))) => {
                                            let mut sum_sq = (r1 - r2).powi(2)
                                                + (g1 - g2).powi(2)
                                                + (b1 - b2).powi(2);
                                            // Only factor alpha in when BOTH sides actually
                                            // specified one - a bare "rgb(...)" comparison
                                            // target (e.g. "is this close to black") has no
                                            // alpha opinion at all, so it shouldn't silently
                                            // get treated as fully opaque.
                                            if let (Some(a1), Some(a2)) = (a1, a2) {
                                                sum_sq += (a1 - a2).powi(2);
                                            }
                                            JsResult::Ok(JsValue::new(sum_sq.sqrt()))
                                        }
                                        // "none" (or anything unparseable) isn't a color to
                                        // compare - report it as maximally far away (real RGB
                                        // distances top out around 441.67, black to white)
                                        // rather than crashing or silently matching everything.
                                        _ => JsResult::Ok(JsValue::new(f64::MAX)),
                                    }
                                }
                                _ => JsResult::Ok(JsValue::new(f64::MAX)),
                            }
                        },
                    ),
                );
            }
            {
                // No geometry, no new shapes - concatenates every slot
                // under `label` into ONE slot at (out_label, out_index).
                // This is the explicit form of the "free union" a shared
                // slot already gives you: filtering `main` (one shape per
                // slot) produces one slot per match, so anything that
                // needs the WHOLE matched set as a single operand (offset,
                // union, intersect, difference all resolve just one slot
                // per operand) needs this first.
                let groups = groups.clone();
                let _ = context.register_global_callable(
                    "flatten".into(),
                    0,
                    NativeFunction::from_closure(
                        move |_this: &JsValue, args: &[JsValue], _context: &mut Context| {
                            let mut iter = args.iter().peekable();
                            let Some(label) = take_label(&mut iter) else {
                                return JsResult::Ok(JsValue::new(0));
                            };
                            let Some(out_label) = take_label(&mut iter) else {
                                return JsResult::Ok(JsValue::new(0));
                            };
                            let out_index = take_optional_index(&mut iter);

                            let count = flatten_slot(&groups, &label, &out_label, out_index);
                            JsResult::Ok(JsValue::new(count as i64))
                        },
                    ),
                );
            }
            {
                use geo_clipper::Clipper;
                let shapes = shapes.clone();
                let depths = depths.clone();
                let groups = groups.clone();
                let _ = context.register_global_callable(
                    "offset".into(),
                    0,
                    NativeFunction::from_closure(
                        move |_this: &JsValue, args: &[JsValue], context: &mut Context| {
                            let mut iter = args.iter().peekable();
                            let Some(in_label) = take_label(&mut iter) else {
                                return JsResult::Ok(JsValue::new(0));
                            };
                            let in_index = take_optional_index(&mut iter);
                            let Some(out_label) = take_label(&mut iter) else {
                                return JsResult::Ok(JsValue::new(0));
                            };
                            let out_index = take_optional_index(&mut iter);
                            let Some(distance) = take_number(&mut iter) else {
                                return JsResult::Ok(JsValue::new(0));
                            };
                            let join_type_name = take_optional_string(&mut iter, "round");
                            // A round join can no longer default to `2.0`:
                            // `join_type_from` scales that by CLIPPER_FACTOR, so
                            // it would now mean 2 inches instead of the near
                            // -infinitely-fine value it used to mean. The
                            // instruction form resolves from a setting instead;
                            // a bare call has to say what it wants.
                            let join_type_value = match iter.peek() {
                                Some(JsValue::Integer(_)) | Some(JsValue::Rational(_)) => {
                                    take_optional_number(&mut iter, 0.0)
                                }
                                _ if join_type_name == "square" => 0.0,
                                // Fall back to the same program-wide setting
                                // the Offset instruction resolves, so setting
                                // it once at the top of a program fixes both
                                // call styles. Without this the two disagree:
                                // every ivy query calling offset() without a
                                // join value fails outright, and an error
                                // tells you to supply a value the program
                                // already supplied.
                                _ => {
                                    let setting = match join_type_name.as_str() {
                                        "miter" => "miter_limit",
                                        _ => "arc_tolerance",
                                    };
                                    let found = context
                                        .eval(boa_engine::Source::from_bytes(&format!(
                                            "typeof {setting} === 'number' ? {setting} : -1"
                                        )))
                                        .ok()
                                        .and_then(|value| value.as_number())
                                        .filter(|value| value.is_finite() && *value > 0.0);

                                    match found {
                                        Some(value) => value,
                                        None => {
                                            let what = match join_type_name.as_str() {
                                                "miter" => "a miter limit (dimensionless)",
                                                _ => "an arc_tolerance in drawing units",
                                            };
                                            let message = format!(
                                                "offset: a {join_type_name} join needs {what} - \
                                                 pass it as the last argument, or set it once for \
                                                 the program: {setting} = 0.1 * 0.001;"
                                            );
                                            return JsResult::Err(JsError::from_opaque(
                                                js_string!(message).into(),
                                            ));
                                        }
                                    }
                                }
                            };
                            if join_type_name == "round"
                                && !(join_type_value.is_finite() && join_type_value > 0.0)
                            {
                                return JsResult::Err(JsError::from_opaque(
                                    js_string!("offset: arc_tolerance must be a positive length in drawing units").into(),
                                ));
                            }

                            let count = offset_slot(
                                &shapes, &depths, &groups,
                                &in_label, in_index, &out_label, out_index,
                                distance, &join_type_name, join_type_value,
                            );
                            JsResult::Ok(JsValue::new(count as i64))
                        },
                    ),
                );
            }
            {
                use geo_clipper::Clipper;
                fn boolean_op(
                    shapes: &Arc<Mutex<Vec<Polygon>>>,
                    depths: &Arc<Mutex<Vec<usize>>>,
                    groups: &Arc<Mutex<HashMap<String, Vec<Vec<usize>>>>>,
                    args: &[JsValue],
                    op: fn(&MultiPolygon, &MultiPolygon, f64) -> MultiPolygon,
                ) -> JsResult<JsValue> {
                    let mut iter = args.iter().peekable();
                    let Some(a_label) = take_label(&mut iter) else { return JsResult::Ok(JsValue::new(0)) };
                    let a_index = take_optional_index(&mut iter);
                    let Some(b_label) = take_label(&mut iter) else { return JsResult::Ok(JsValue::new(0)) };
                    let b_index = take_optional_index(&mut iter);
                    let Some(out_label) = take_label(&mut iter) else { return JsResult::Ok(JsValue::new(0)) };
                    let out_index = take_optional_index(&mut iter);

                    let count = boolean_slot(
                        shapes, depths, groups,
                        &a_label, a_index, &b_label, b_index, &out_label, out_index, op,
                    );
                    JsResult::Ok(JsValue::new(count as i64))
                }

                {
                    let shapes = shapes.clone();
                    let depths = depths.clone();
                    let groups = groups.clone();
                    let _ = context.register_global_callable(
                        "union".into(),
                        0,
                        NativeFunction::from_closure(move |_this: &JsValue, args: &[JsValue], _context: &mut Context| {
                            boolean_op(&shapes, &depths, &groups, args, |a, b, f| Clipper::union(a, b, f))
                        }),
                    );
                }

                {
                    let shapes = shapes.clone();
                    let depths = depths.clone();
                    let groups = groups.clone();
                    let _ = context.register_global_callable(
                        "intersect".into(),
                        0,
                        NativeFunction::from_closure(move |_this: &JsValue, args: &[JsValue], _context: &mut Context| {
                            boolean_op(&shapes, &depths, &groups, args, |a, b, f| Clipper::intersection(a, b, f))
                        }),
                    );
                }

                {
                    let shapes = shapes.clone();
                    let depths = depths.clone();
                    let groups = groups.clone();
                    let _ = context.register_global_callable(
                        "difference".into(),
                        0,
                        NativeFunction::from_closure(move |_this: &JsValue, args: &[JsValue], _context: &mut Context| {
                            boolean_op(&shapes, &depths, &groups, args, |a, b, f| Clipper::difference(a, b, f))
                        }),
                    );
                }
            }
        }

        eprintln!("Build Data");
        (
            Self {
                shapes,
                depths,
                groups,
                context,
                shape_data,
                functions: Arc::new(Mutex::new(HashMap::new())),
                call_depth: Arc::new(Mutex::new(0)),
                current_group: current_group_field,
                warnings: Arc::new(Mutex::new(Vec::new())),
                debug: false,
                annotations: Arc::new(Mutex::new(Vec::new())),
            },
            indexes,
        )
    }

    /// Loads an SVG the same way `From<Box<Path>>` does, but also captures
    /// each shape's paint/visibility info (see `depth_tree::ShapeStyle`),
    /// reordered to match the tree's final shape order via the same
    /// `indexes` mapping `from_respect_indexes` already produces - so
    /// `fill_color(i)`/`stroke_color(i)`/`is_visible(i)` are populated
    /// instead of defaulting to "no info".
    pub fn from_svg_path_with_style(path: &std::path::Path) -> Self {
        let pairs = depth_tree::import_svg_with_style(path, 0.0001).unwrap();
        let (lines, styles): (Vec<_>, Vec<depth_tree::ShapeStyle>) = pairs.into_iter().unzip();

        let polygons: Vec<Polygon> = lines
            .into_iter()
            .map(|line| Polygon::new(line, Vec::new()))
            .collect();

        let (data, indexes) = Self::from_respect_indexes(polygons);

        // The SVG's own stroke widths aren't carried through - a cut file
        // wants a hairline regardless, and a program sets one where it matters.
        *data.shape_data.lock().unwrap() = indexes
            .iter()
            .map(|&i| {
                let mut map = serde_json::Map::new();
                map.insert("fill".into(), rgba_to_js_string(styles[i].fill_rgba).into());
                map.insert("stroke".into(), rgba_to_js_string(styles[i].stroke_rgba).into());
                map.insert("visible".into(), styles[i].visible.into());
                map
            })
            .collect();

        data
    }
}
