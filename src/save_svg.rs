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
