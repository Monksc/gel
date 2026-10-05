use geo::{BoundingRect, MultiPolygon, Polygon, Scale, Translate};

/// Convert a geo::Polygon<f64> into an SVG path string
fn polygon_to_svg_path(polygon: &Polygon<f64>) -> String {
    let mut d = String::new();

    // Exterior ring
    if let Some(first) = polygon.exterior().points().next() {
        d += &format!("M {} {}", first.x(), first.y());
        for p in polygon.exterior().points().skip(1) {
            d += &format!(" L {} {}", p.x(), p.y());
        }
        d += " Z"; // close path
    }

    // Interior rings (holes)
    for interior in polygon.interiors() {
        if let Some(first) = interior.points().next() {
            d += &format!(" M {} {}", first.x(), first.y());
            for p in interior.points().skip(1) {
                d += &format!(" L {} {}", p.x(), p.y());
            }
            d += " Z";
        }
    }

    d
}

/// Convert multiple polygons into a full SVG document
pub fn polygons_to_svg(polygons: &[Polygon<f64>]) -> String {
    let mut polygons = MultiPolygon::new(polygons.iter().map(|polygon| polygon.clone()).collect());

    polygons.scale_xy_mut(1.0, -1.0);
    let frame = polygons.bounding_rect().unwrap();
    polygons.translate_mut(-frame.min().x, -frame.min().y);

    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}in" height="{}in" viewBox="0 0 {} {}">"#,
        frame.width(),
        frame.height(),
        frame.width(),
        frame.height(),
    );

    for poly in polygons.0 {
        let path_data = polygon_to_svg_path(&poly);
        svg += &format!(
            r#"<path d="{}" fill="none" stroke="black" stroke-width="0.0005in"/>"#,
            path_data
        );
    }

    svg += "</svg>";
    svg
}

/// Same viewBox/transform handling as `polygons_to_svg`, but each polygon
/// keeps its own fill/stroke instead of the forced `fill=none stroke=black`
/// outline look - for a "what does the finished sign actually look like"
/// render rather than a toolpath/outline one. `styles[i]` is
/// `(fill, stroke)`, each either a `"rgba(r,g,b,a)"` string (as produced by
/// gel's `fill_color`/`stroke_color`) or `None` for `"none"`.
/// How one shape should be painted, plus whatever else its data map carried.
///
/// `extra` becomes `data-*` attributes. That is what makes the open shape
/// store useful beyond paint: a milling tool number or a 3-D print
/// orientation survives into the emitted file for a downstream step to read,
/// without `Emit` needing to know what it means.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct PathStyle {
    pub fill: Option<String>,
    pub stroke: Option<String>,
    pub stroke_width: Option<String>,
    /// Sorted, so two shapes with the same data group together.
    pub extra: Vec<(String, String)>,
}

pub fn polygons_to_svg_styled(polygons: &[Polygon<f64>], styles: &[PathStyle]) -> String {
    assert_eq!(polygons.len(), styles.len(), "polygons_to_svg_styled: styles must be 1:1 with polygons");

    let mut polygons = MultiPolygon::new(polygons.iter().map(|polygon| polygon.clone()).collect());

    polygons.scale_xy_mut(1.0, -1.0);
    let frame = polygons.bounding_rect().unwrap();
    polygons.translate_mut(-frame.min().x, -frame.min().y);

    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}in" height="{}in" viewBox="0 0 {} {}">"#,
        frame.width(),
        frame.height(),
        frame.width(),
        frame.height(),
    );

    // Group shapes by (fill, stroke) and emit ONE <path> per group with
    // fill-rule="evenodd", instead of one <path> per shape. A glyph's outer
    // contour and its inner counter are separate shapes here (gel doesn't
    // store nested holes as one Polygon's interior rings - depth_tree
    // tracks nesting separately), but they typically share the same fill.
    // Drawing them as independent opaque paths paints the counter solid
    // instead of punching a hole through the contour; combining same-style
    // shapes into one evenodd path makes nested same-color regions
    // alternate fill/hole correctly - the standard way to render vector
    // art with holes, not a hack specific to text.
    let mut order: Vec<PathStyle> = Vec::new();
    let mut path_data_by_style: std::collections::HashMap<PathStyle, String> =
        std::collections::HashMap::new();
    for (poly, style) in polygons.0.iter().zip(styles.iter()) {
        let path_data = polygon_to_svg_path(poly);
        let entry = path_data_by_style.entry(style.clone()).or_insert_with(|| {
            order.push(style.clone());
            String::new()
        });
        if !entry.is_empty() {
            entry.push(' ');
        }
        entry.push_str(&path_data);
    }

    for style in order {
        let d = &path_data_by_style[&style];
        let fill_attr = style.fill.as_deref().unwrap_or("none");
        svg += &format!(r#"<path d="{}" fill="{}" fill-rule="evenodd""#, d, fill_attr);
        for (key, value) in &style.extra {
            svg += &format!(r#" data-{}="{}""#, key, value);
        }
        if let Some(stroke) = &style.stroke {
            // No explicit width means a hairline, which is what a cut file
            // wants - the line marks a path, it doesn't have a thickness.
            let width = style.stroke_width.as_deref().unwrap_or("0.0005in");
            svg += &format!(r#" stroke="{}" stroke-width="{}""#, stroke, width);
        }
        svg += "/>";
    }

    svg += "</svg>";
    svg
}
