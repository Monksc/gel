//! Output instructions: `AddText` and `Emit`.
//!
//! `AddText` records a label for the operator; `Emit` writes a group's
//! geometry to an SVG, optionally including those labels.
//!
//! Labels are annotations, not geometry, because a label is never cut.
//! Keeping them out of the shape store means gel needs no font handling at
//! all - the SVG viewer renders a `<text>` element - and it means one
//! program over one set of geometry produces both the cut file and the
//! annotated "verbose" copy, differing only by whether `Emit` was asked for
//! annotations.

use boa_engine::{JsValue, Source};
use serde::{Deserialize, Serialize};

use crate::*;

/// Records a label for the operator. Writes no geometry.
///
/// Position is either explicit (`x`/`y`) or taken from a group's frame via
/// `anchor_group`, which is how a per-sheet header gets placed without
/// knowing where nesting put the sheet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddText {
    /// Expression producing the label text.
    pub text: String,
    #[serde(default = "default_font")]
    pub font: String,
    /// Expression. Defaults to 0.25 (inches).
    #[serde(default)]
    pub size: Option<String>,
    /// Expression. Ignored when `anchor_group` is set.
    #[serde(default)]
    pub x: Option<String>,
    /// Expression. Ignored when `anchor_group` is set.
    #[serde(default)]
    pub y: Option<String>,
    /// Place the label at this group's frame instead of explicit x/y.
    #[serde(default)]
    pub anchor_group: Option<String>,
    #[serde(default)]
    pub anchor_index: Index,
    /// Which corner of `anchor_group`'s frame to sit at: `"top_left"`
    /// (default), `"top_right"`, `"bottom_left"`, `"bottom_right"`.
    #[serde(default)]
    pub corner: Option<String>,
    /// CSS colour for this label. Defaults to black.
    #[serde(default)]
    pub color: Option<String>,
}

fn default_font() -> String {
    "sans-serif".into()
}

/// How far above the geometry a top-anchored label's baseline sits, in
/// multiples of the font size. Enough of a gap that the header reads as a
/// caption for the sheet rather than as part of it.
const HEADER_GAP: f64 = 1.5;

impl Query for AddText {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let text = eval_string(data, &self.text, "AddText.text")?;
        let size = match &self.size {
            Some(expression) => eval_number(data, expression, "AddText.size")?,
            None => 0.25,
        };

        let (x, y) = match &self.anchor_group {
            Some(group) => {
                let group = resolve_template(data, group, "AddText.anchor_group")?;
                let anchor_index = self.anchor_index.resolve(data, "AddText.anchor_index")?;
                let frame = {
                    let groups = data.groups.lock().unwrap();
                    let shapes = data.shapes.lock().unwrap();
                    let indices = resolve_indices(&groups, &group, anchor_index);
                    if indices.is_empty() {
                        return Err(format!(
                            "AddText.anchor_group '{group}' slot {anchor_index} is empty"
                        ));
                    }
                    use geo::BoundingRect;
                    multipolygon_from_indices(&shapes, &indices)
                        .bounding_rect()
                        .ok_or_else(|| format!("AddText.anchor_group '{group}' has no extent"))?
                };
                // A top corner sits ABOVE the geometry, not on it - a header
                // that overlaps the parts is unreadable, and one placed
                // exactly on the top edge has its glyphs drawn outside the
                // page (SVG `y` is the baseline) where they vanish entirely.
                // `Emit` grows the page upward to make room for whatever
                // lands here.
                let above = frame.max().y + HEADER_GAP * size;
                match self.corner.as_deref().unwrap_or("top_left") {
                    "top_right" => (frame.max().x, above),
                    "bottom_left" => (frame.min().x, frame.min().y),
                    "bottom_right" => (frame.max().x, frame.min().y),
                    _ => (frame.min().x, above),
                }
            }
            None => {
                let x = match &self.x {
                    Some(expression) => eval_number(data, expression, "AddText.x")?,
                    None => 0.0,
                };
                let y = match &self.y {
                    Some(expression) => eval_number(data, expression, "AddText.y")?,
                    None => 0.0,
                };
                (x, y)
            }
        };

        data.annotations.lock().unwrap().push(Annotation {
            text,
            font: self.font.clone(),
            size,
            color: self.color.clone().unwrap_or_else(|| "black".into()),
            x,
            y,
        });
        Ok(())
    }
}

/// Writes a group's geometry to an SVG file.
///
/// `styled` keeps each shape's own colours (for a proof); the default is a
/// black hairline with no fill, which is what a cut file wants.
///
/// `face` is recorded for the operator only - "Specify Face UP or Face DOWN
/// cuts" in the Laser guide - and never mirrors the geometry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Emit {
    pub get_group: String,
    /// Emit only this slot of the group, rather than all of it.
    ///
    /// Needed because slots are meaningful: `Nest` writes one slot per
    /// sheet, all in the same stock coordinates, so flattening them into one
    /// file draws every sheet on top of every other.
    #[serde(default)]
    pub get_index: Option<Index>,
    /// Path to write. Parent directories are created. `{expression}`
    /// expands, so a loop can write one file per sheet.
    pub out: String,
    /// Preserve each shape's own fill/stroke instead of a black hairline.
    #[serde(default)]
    pub styled: bool,
    /// Include the labels `AddText` recorded. Off for a cut file, on for
    /// the verbose copy.
    #[serde(default)]
    pub annotations: bool,
    /// `"up"` / `"down"`, informational only.
    #[serde(default)]
    pub face: Option<String>,
}

/// Turns a stored colour into one SVG will accept, or `None` for no paint.
///
/// gel's own `rgba(r,g,b,a)` uses a 0-255 alpha, which CSS does not, so that
/// form is converted. Anything else a program wrote (`red`, `#ff0000`) is
/// validated and passed through - validated because SVG silently renders an
/// unrecognised colour as black, which on a proof looks like a decision
/// rather than a typo.
fn css_color_string(text: &str) -> Result<Option<String>, String> {
    let rgba = parse_color(text)?;
    Ok(match rgba {
        None => None,
        Some((r, g, b, 255)) => Some(format!("rgb({r},{g},{b})")),
        Some((r, g, b, a)) => Some(format!("rgba({r},{g},{b},{})", a as f64 / 255.0)),
    })
}

impl Query for Emit {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let get_group = resolve_template(data, &self.get_group, "Emit.get_group")?;
        let get_index = match &self.get_index {
            Some(index) => Some(index.resolve(data, "Emit.get_index")?),
            None => None,
        };
        let out = resolve_template(data, &self.out, "Emit.out")?;

        let indices: Vec<usize> = {
            let groups = data.groups.lock().unwrap();
            let entries = groups
                .get(&get_group)
                .ok_or_else(|| format!("Emit: unknown group '{get_group}'"))?;
            match get_index {
                Some(index) => entries
                    .get(index)
                    .ok_or_else(|| format!(
                        "Emit: group '{get_group}' has no slot {index} (it has {})",
                        entries.len()))?
                    .clone(),
                None => entries.iter().flatten().copied().collect(),
            }
        };
        if indices.is_empty() {
            return Err(format!("Emit: group '{get_group}' has no shapes"));
        }

        let (polygons, styles) = {
            let shapes = data.shapes.lock().unwrap();
            let store = data.shape_data.lock().unwrap();
            let empty = serde_json::Map::new();

            let mut polygons: Vec<geo::Polygon> = Vec::new();
            let mut styles: Vec<PathStyle> = Vec::new();
            for &i in &indices {
                let shape_data = store.get(i).unwrap_or(&empty);

                // An explicit `visible: false` means don't emit it at all -
                // an invisible shape should not become a cut line.
                if shape_data.get("visible") == Some(&serde_json::Value::Bool(false)) {
                    continue;
                }

                let style = if self.styled {
                    let mut style = PathStyle::default();
                    for (key, value) in shape_data {
                        match key.as_str() {
                            "fill" | "stroke" => {
                                let Some(text) = value.as_str() else {
                                    return Err(format!(
                                        "Emit: shape {key} is {value}, which is not a colour string"
                                    ));
                                };
                                let color = css_color_string(text)
                                    .map_err(|err| format!("Emit: shape {key}: {err}"))?;
                                if key == "fill" {
                                    style.fill = color;
                                } else {
                                    style.stroke = color;
                                }
                            }
                            "stroke_width" => {
                                let Some(width) = value.as_f64() else {
                                    return Err(format!(
                                        "Emit: stroke_width is {value}, which is not a number"
                                    ));
                                };
                                style.stroke_width = Some(format!("{width}in"));
                            }
                            "visible" => {}
                            // Anything gel doesn't interpret rides along as a
                            // data-* attribute rather than being dropped.
                            other => {
                                let text = match value {
                                    serde_json::Value::String(s) => s.clone(),
                                    other => other.to_string(),
                                };
                                style.extra.push((other.to_string(), escape_xml(&text)));
                            }
                        }
                    }
                    style.extra.sort();
                    style
                } else {
                    // A cut file is a set of lines, not filled regions.
                    PathStyle {
                        stroke: Some("black".to_string()),
                        ..PathStyle::default()
                    }
                };

                polygons.push(shapes[i].clone());
                styles.push(style);
            }
            (polygons, styles)
        };
        if polygons.is_empty() {
            return Err(format!("Emit: group '{get_group}' has no visible shapes"));
        }

        let mut svg = polygons_to_svg_styled(&polygons, &styles);

        if self.annotations {
            let annotations = data.annotations.lock().unwrap();
            if !annotations.is_empty() {
                // polygons_to_svg_styled flips y and moves the geometry so
                // its own minimum sits at the origin. Annotations are
                // recorded in shape coordinates, so they need the same
                // treatment or they'd land off-page.
                use geo::BoundingRect;
                let frame = geo::MultiPolygon::new(polygons.clone())
                    .bounding_rect()
                    .ok_or("Emit: geometry has no extent")?;
                let mut text_elements = String::new();
                // Most-negative y reached by any glyph, so the page can be
                // grown to fit. Ascenders rise roughly one font size above
                // the baseline.
                let mut headroom: f64 = 0.0;
                for annotation in annotations.iter() {
                    // Mirrors what polygons_to_svg_styled does to the
                    // geometry: flip y (y -> -y), then shift so the flipped
                    // minimum (-max_y) sits at the origin. Composed, that is
                    // simply `max_y - y`.
                    let x = annotation.x - frame.min().x;
                    let y = frame.max().y - annotation.y;
                    headroom = headroom.min(y - annotation.size);
                    text_elements += &format!(
                        r#"<text x="{x}" y="{y}" font-family="{}" font-size="{}" fill="{}">{}</text>"#,
                        annotation.font,
                        annotation.size,
                        annotation.color,
                        escape_xml(&annotation.text),
                    );
                }
                svg = svg.replace("</svg>", &format!("{text_elements}</svg>"));

                // polygons_to_svg_styled sizes the page to the geometry
                // exactly, so anything placed above it (y < 0) falls outside
                // the viewBox and never renders. Grow the page upward to
                // cover the labels instead of silently dropping them.
                if headroom < 0.0 {
                    svg = grow_page_upward(&svg, -headroom);
                }
            }
        }

        if let Some(parent) = std::path::Path::new(&out).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Emit: could not create {}: {e}", parent.display()))?;
            }
        }
        std::fs::write(&out, svg)
            .map_err(|e| format!("Emit: could not write {out}: {e}"))?;
        Ok(())
    }
}

/// Extends an SVG's page upward by `amount`, so content at negative y is
/// inside the viewBox. Only the height and the viewBox's y-origin change -
/// the geometry keeps its coordinates and does not move on the page.
///
/// Depends on the header `polygons_to_svg_styled` writes; if that format
/// changes, this returns the document untouched rather than corrupting it.
fn grow_page_upward(svg: &str, amount: f64) -> String {
    let Some(view_box_start) = svg.find(r#"viewBox="0 0 "#) else {
        return svg.to_string();
    };
    let inner_start = view_box_start + r#"viewBox=""#.len();
    let Some(inner_len) = svg[inner_start..].find('"') else {
        return svg.to_string();
    };
    let inner = &svg[inner_start..inner_start + inner_len];
    let parts: Vec<&str> = inner.split_whitespace().collect();
    let (Some(width), Some(height)) = (
        parts.get(2).and_then(|v| v.parse::<f64>().ok()),
        parts.get(3).and_then(|v| v.parse::<f64>().ok()),
    ) else {
        return svg.to_string();
    };

    let new_height = height + amount;
    let with_box = format!(
        "{}viewBox=\"0 {} {} {}\"{}",
        &svg[..view_box_start],
        -amount,
        width,
        new_height,
        &svg[inner_start + inner_len + 1..],
    );
    with_box.replace(
        &format!(r#"height="{height}in""#),
        &format!(r#"height="{new_height}in""#),
    )
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use geo::polygon;

    use crate::*;

    fn square() -> geo::Polygon {
        polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 0.0), (x: 2.0, y: 2.0), (x: 0.0, y: 2.0)
        ]
    }

    fn out_path(name: &str) -> String {
        std::env::temp_dir()
            .join(format!("gel_emit_test_{name}.svg"))
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn writes_an_svg_for_the_group() {
        let path = out_path("basic");
        let _ = std::fs::remove_file(&path);
        let mut data = Data::from(vec![square()]);

        data.query(vec![Instruction::Emit(Emit {
            get_group: "main".into(),
            get_index: None,
            out: path.clone(),
            styled: false,
            annotations: false,
            face: None,
        })])
        .expect("emit should succeed");

        let svg = std::fs::read_to_string(&path).expect("file should exist");
        assert!(svg.starts_with("<svg"), "got: {}", &svg[..40.min(svg.len())]);
        assert!(svg.contains("<path"), "should contain geometry");
        // A cut file is lines, not filled regions.
        assert!(svg.contains(r#"stroke="black""#), "got: {svg}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn annotations_appear_only_when_asked() {
        let cut = out_path("cut");
        let verbose = out_path("verbose");
        let _ = std::fs::remove_file(&cut);
        let _ = std::fs::remove_file(&verbose);
        let mut data = Data::from(vec![square()]);

        // One program, one set of geometry, two files - the whole point of
        // annotations being a flag rather than a separate program.
        data.query(vec![
            Instruction::RunCode(RunCode {
                code: "sheet = 2; total = 7;".into(),
            }),
            Instruction::AddText(AddText {
                text: "'Sheet ' + sheet + ' of ' + total".into(),
                font: "sans-serif".into(),
                size: None,
                x: Some("0".into()),
                y: Some("0".into()),
                anchor_group: None,
                anchor_index: Index::Literal(0),
                corner: None,
                color: None,
            }),
            Instruction::Emit(Emit {
                get_group: "main".into(),
                get_index: None,
                out: cut.clone(),
                styled: false,
                annotations: false,
                face: Some("up".into()),
            }),
            Instruction::Emit(Emit {
                get_group: "main".into(),
                get_index: None,
                out: verbose.clone(),
                styled: false,
                annotations: true,
                face: Some("up".into()),
            }),
        ])
        .expect("program should run");

        let cut_svg = std::fs::read_to_string(&cut).unwrap();
        let verbose_svg = std::fs::read_to_string(&verbose).unwrap();

        assert!(!cut_svg.contains("<text"), "cut file must carry no labels");
        assert!(verbose_svg.contains("<text"), "verbose file must carry labels");
        // The label is an expression, so it reads what RunCode defined.
        assert!(verbose_svg.contains("Sheet 2 of 7"), "got: {verbose_svg}");

        let _ = std::fs::remove_file(&cut);
        let _ = std::fs::remove_file(&verbose);
    }

    #[test]
    fn anchor_group_places_the_label_without_knowing_coordinates() {
        let path = out_path("anchored");
        let _ = std::fs::remove_file(&path);
        let mut data = Data::from(vec![square()]);

        data.query(vec![
            Instruction::AddText(AddText {
                text: "'header'".into(),
                font: "sans-serif".into(),
                size: None,
                x: None,
                y: None,
                anchor_group: Some("main".into()),
                anchor_index: Index::Literal(0),
                corner: Some("top_left".into()),
                color: None,
            }),
            Instruction::Emit(Emit {
                get_group: "main".into(),
                get_index: None,
                out: path.clone(),
                styled: false,
                annotations: true,
                face: None,
            }),
        ])
        .expect("program should run");

        assert!(std::fs::read_to_string(&path).unwrap().contains("header"));
        let _ = std::fs::remove_file(&path);
    }

    /// A top_left anchor must put the label at the TOP of the emitted SVG,
    /// and INSIDE it. Caught two real bugs: the y-flip was composed wrongly
    /// so every label landed at the far edge (y=2 on a 2-unit square), and
    /// once fixed, a baseline of exactly 0 drew the glyphs above the viewBox
    /// where they were silently invisible.
    #[test]
    fn a_top_anchored_label_lands_at_the_top() {
        let path = out_path("top_anchor");
        let _ = std::fs::remove_file(&path);
        // A square spanning y = -5 .. -3, i.e. the negative-y space real
        // drawings land in after import.
        let mut data = Data::from(vec![polygon![
            (x: 0.0, y: -5.0), (x: 2.0, y: -5.0), (x: 2.0, y: -3.0), (x: 0.0, y: -3.0)
        ]]);

        data.query(vec![
            Instruction::AddText(AddText {
                text: "'header'".into(),
                font: "sans-serif".into(),
                size: None,
                x: None,
                y: None,
                anchor_group: Some("main".into()),
                anchor_index: Index::Literal(0),
                corner: Some("top_left".into()),
                color: None,
            }),
            Instruction::Emit(Emit {
                get_group: "main".into(),
                get_index: None,
                out: path.clone(),
                styled: false,
                annotations: true,
                face: None,
            }),
        ])
        .expect("program should run");

        let svg = std::fs::read_to_string(&path).unwrap();
        let y: f64 = svg
            .split(r#"<text x=""#)
            .nth(1)
            .and_then(|rest| rest.split(r#"y=""#).nth(1))
            .and_then(|rest| rest.split('"').next())
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("no <text y=> in {svg}"));
        // The shape is 2 tall, occupying y = 0..2. A header belongs ABOVE
        // it, so the baseline is negative - 1.5 font sizes (the 0.25
        // default) clear of the top edge.
        assert!((y - -0.375).abs() < 1e-6, "expected baseline at y=-0.375, got {y}");
        assert!(y < 0.0, "a header must sit above the geometry, got {y}");

        // ...and the page must have grown to include it, or it renders
        // outside the viewBox and is invisible.
        let view_box = svg
            .split(r#"viewBox=""#)
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("should have a viewBox");
        let top: f64 = view_box.split_whitespace().nth(1).unwrap().parse().unwrap();
        assert!(top <= y - 0.25 + 1e-9,
            "viewBox top {top} must cover the label's glyphs (baseline {y})");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_unknown_group_is_an_error() {
        let mut data = Data::from(vec![square()]);
        let result = data.query(vec![Instruction::Emit(Emit {
            get_group: "no_such_group".into(),
            get_index: None,
            out: out_path("never"),
            styled: false,
            annotations: false,
            face: None,
        })]);
        assert!(result.is_err(), "emitting an unknown group should fail loudly");
    }
}
