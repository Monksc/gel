//! MCP server for inspecting/querying `gel`-loadable drawings (SVG, or CDR
//! once converted the same way asg-quote-cost's cdrConverter.js does) - the
//! "let Claude feel the .cdr files" tool from AUTOMATION_ROADMAP.md's Sep
//! Wk1 file-identification work. Read-only exploration today (load/filter/
//! group_by/sort/stats/list_shapes over gel's existing query pipeline);
//! `nest` is a placeholder until a real nesting crate is evaluated.
//!
//! This is deliberately separate from the "MCP server for gel" planned in
//! AUTOMATION_ROADMAP.md §1.5 for driving production layout runs later -
//! this one is just for interactive inspection right now.

mod nest;
mod worker;

use rmcp::{
    ErrorData, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::io::stdio,
};
use schemars::JsonSchema;
use serde::Deserialize;
use worker::{Command, WorkerHandle};

#[derive(Debug, Deserialize, JsonSchema)]
struct LoadSvgRequest {
    #[schemars(description = "absolute path to an SVG file (convert CDR to SVG first)")]
    path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct FilterRequest {
    #[schemars(description = "handle returned by load_svg")]
    handle: String,
    #[schemars(description = "existing group to read from, e.g. \"main\"")]
    get_group: String,
    #[schemars(description = "name for the resulting group")]
    set_group: String,
    #[schemars(
        description = "boa JS boolean expression over shape index `i`, e.g. \"depth(i) % 2 == 0 && depth(i) > 1\". Also available: area(i), center(i), circle_metrics(i).circle, distance(i, j)."
    )]
    code: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GroupByRequest {
    handle: String,
    get_group: String,
    set_group: String,
    #[schemars(
        description = "boa JS boolean expression deciding whether index `i` joins the group already anchored by index `j`, e.g. \"distance(i, j) < 5\""
    )]
    code: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SortRequest {
    handle: String,
    get_group: String,
    set_group: String,
    #[schemars(description = "boa JS comparator over indexes `l`/`r`, e.g. \"area(l) - area(r)\"")]
    compare: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GroupRequest {
    handle: String,
    group: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListShapesRequest {
    handle: String,
    group: String,
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    50
}

#[derive(Debug, Deserialize, JsonSchema)]
struct NestRequest {
    #[schemars(description = "handle returned by load_svg")]
    handle: String,
    #[schemars(description = "group whose shapes to nest - one nest item per shape index, group entries are flattened")]
    group: String,
    #[schemars(description = "sheet width to nest onto")]
    sheet_width: f64,
    #[schemars(description = "sheet height to nest onto")]
    sheet_height: f64,
    #[schemars(description = "minimum spacing/kerf margin to enforce between items (and between items and the sheet edge); omit for none")]
    min_item_separation: Option<f64>,
    #[schemars(description = "allow items to rotate (default false - asgdraw.txt's 'Preserve Orientation')")]
    #[serde(default)]
    allow_rotation: bool,
    #[schemars(description = "directory to write one SVG per sheet into (sheet_0.svg, sheet_1.svg, ...)")]
    out_dir: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct FilterSvgRequest {
    #[schemars(description = "raw SVG file to filter (e.g. straight from a CDR->SVG conversion)")]
    in_path: String,
    #[schemars(description = "path to write the cleaned SVG to")]
    out_path: String,
    #[schemars(description = "custom (attr, value) rules - elements with a matching attribute are stripped along with their subtree. Omit to use the known default rules (currently just LibreOffice's class=\"BoundingBox\" helper rectangles).")]
    #[serde(default)]
    rules: Vec<(String, String)>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RenderGroupRequest {
    #[schemars(description = "handle returned by load_svg")]
    handle: String,
    #[schemars(description = "group to render")]
    group: String,
    #[schemars(description = "file path to write the rendered SVG to")]
    out_path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ShapeInfoRequest {
    #[schemars(description = "handle returned by load_svg")]
    handle: String,
    #[schemars(description = "shape index")]
    index: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TranslateAndRenderRequest {
    #[schemars(description = "handle returned by load_svg")]
    handle: String,
    #[schemars(description = "group to translate and render")]
    group: String,
    #[schemars(description = "x offset to apply to every shape in the group")]
    dx: f64,
    #[schemars(description = "y offset to apply to every shape in the group")]
    dy: f64,
    #[schemars(description = "file path to write the rendered SVG to")]
    out_path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TranslateGroupsAndRenderRequest {
    #[schemars(description = "handle returned by load_svg")]
    handle: String,
    #[schemars(description = "list of [group, dx, dy] triples - each group's shapes are translated by its own dx/dy, then all combined into one rendered SVG")]
    translations: Vec<(String, f64, f64)>,
    #[schemars(description = "one entry per physical plate: (offset_x, offset_y, width, height) - draws a plain rectangle at that offset, so several plates can share ONE output file instead of one file per plate")]
    sheet_borders: Vec<(f64, f64, f64, f64)>,
    #[schemars(description = "file path to write the combined rendered SVG to")]
    out_path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TranslateGroupsAndRenderStyledRequest {
    #[schemars(description = "handle returned by load_svg")]
    handle: String,
    #[schemars(description = "list of [group, dx, dy] triples - each group's shapes are translated by its own dx/dy, then all combined into one rendered SVG")]
    translations: Vec<(String, f64, f64)>,
    #[schemars(description = "one entry per physical plate: (offset_x, offset_y, width, height) - draws a plain black-outline rectangle at that offset (plate boundaries aren't part of the sign artwork, so they don't get a fill/color)")]
    sheet_borders: Vec<(f64, f64, f64, f64)>,
    #[schemars(description = "file path to write the combined rendered SVG to")]
    out_path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PlaceGroupsAndRenderRequest {
    #[schemars(description = "handle returned by load_svg")]
    handle: String,
    #[schemars(description = "one entry per group to place - see PlacementEntry: rotate around (pivot_x, pivot_y) [the reference shape's own centroid, from shape_info] by rotation_degrees, then translate to `translation` [nest's reported final position for that shape]")]
    placements: Vec<worker::PlacementEntry>,
    #[schemars(description = "one entry per physical plate: (offset_x, offset_y, width, height) - draws a plain rectangle at that offset, so several plates can share ONE output file instead of one file per plate")]
    sheet_borders: Vec<(f64, f64, f64, f64)>,
    #[schemars(description = "file path to write the combined rendered SVG to")]
    out_path: String,
}

#[derive(Clone)]
struct GelInspector {
    worker: std::sync::Arc<WorkerHandle>,
    tool_router: ToolRouter<Self>,
}

fn to_mcp_err(e: String) -> ErrorData {
    ErrorData::internal_error(e, None)
}

#[tool_router]
impl GelInspector {
    fn new() -> Self {
        Self { worker: std::sync::Arc::new(WorkerHandle::spawn()), tool_router: Self::tool_router() }
    }

    #[tool(description = "Load an SVG file (convert CDR to SVG first, same as asg-quote-cost's cdrConverter.js) into a new handle. Returns the handle id and shape count.")]
    async fn load_svg(&self, Parameters(LoadSvgRequest { path }): Parameters<LoadSvgRequest>) -> Result<String, ErrorData> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.send(Command::LoadSvg { path, respond: tx });
        rx.await.map_err(|e| to_mcp_err(e.to_string()))?.map_err(to_mcp_err)
    }

    #[tool(description = "Filter a group by a JS boolean predicate over shape index `i` (depth(i), area(i), center(i), circle_metrics(i), distance(i,j) are available), writing survivors into a new/overwritten group.")]
    async fn filter(&self, Parameters(FilterRequest { handle, get_group, set_group, code }): Parameters<FilterRequest>) -> Result<String, ErrorData> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.send(Command::Filter { handle, get_group, set_group, code, respond: tx });
        let v = rx.await.map_err(|e| to_mcp_err(e.to_string()))?.map_err(to_mcp_err)?;
        Ok(v.to_string())
    }

    #[tool(description = "Cluster a group by a JS predicate deciding whether index `i` joins the cluster anchored by index `j` (e.g. proximity via distance(i,j)).")]
    async fn group_by(&self, Parameters(GroupByRequest { handle, get_group, set_group, code }): Parameters<GroupByRequest>) -> Result<String, ErrorData> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.send(Command::GroupBy { handle, get_group, set_group, code, respond: tx });
        let v = rx.await.map_err(|e| to_mcp_err(e.to_string()))?.map_err(to_mcp_err)?;
        Ok(v.to_string())
    }

    #[tool(description = "Sort a group by a JS comparator over indexes `l`/`r`.")]
    async fn sort(&self, Parameters(SortRequest { handle, get_group, set_group, compare }): Parameters<SortRequest>) -> Result<String, ErrorData> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.send(Command::Sort { handle, get_group, set_group, compare, respond: tx });
        let v = rx.await.map_err(|e| to_mcp_err(e.to_string()))?.map_err(to_mcp_err)?;
        Ok(v.to_string())
    }

    #[tool(description = "Summary stats for a group: shape count, depth histogram, total area. The quick 'feel for the file' overview.")]
    async fn stats(&self, Parameters(GroupRequest { handle, group }): Parameters<GroupRequest>) -> Result<String, ErrorData> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.send(Command::Stats { handle, group, respond: tx });
        let v = rx.await.map_err(|e| to_mcp_err(e.to_string()))?.map_err(to_mcp_err)?;
        Ok(v.to_string())
    }

    #[tool(description = "Per-shape detail (index, depth, area) for a group, capped at `limit` (default 50).")]
    async fn list_shapes(&self, Parameters(ListShapesRequest { handle, group, limit }): Parameters<ListShapesRequest>) -> Result<String, ErrorData> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.send(Command::ListShapes { handle, group, limit, respond: tx });
        let v = rx.await.map_err(|e| to_mcp_err(e.to_string()))?.map_err(to_mcp_err)?;
        Ok(v.to_string())
    }

    #[tool(description = "Nest a group's shapes onto sheet_width x sheet_height sheets (bin-packing, via jagua-rs+lbf - see DESIGN.md). Opens as many sheets as needed. Returns per-sheet SVG paths and per-item placements (translation/rotation).")]
    async fn nest(&self, Parameters(NestRequest { handle, group, sheet_width, sheet_height, min_item_separation, allow_rotation, out_dir }): Parameters<NestRequest>) -> Result<String, ErrorData> {
        let params = serde_json::json!({
            "sheet_width": sheet_width,
            "sheet_height": sheet_height,
            "min_item_separation": min_item_separation,
            "allow_rotation": allow_rotation,
            "out_dir": out_dir,
        });
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.send(Command::Nest { handle, group, params, respond: tx });
        let v = rx.await.map_err(|e| to_mcp_err(e.to_string()))?.map_err(to_mcp_err)?;
        Ok(v.to_string())
    }

    #[tool(description = "Strip synthetic artifact elements from a raw SVG (e.g. LibreOffice's per-shape class=\"BoundingBox\" helper rectangles, which otherwise contaminate depth-based shape classification) and write the cleaned result to a new file. Run this between converting CDR to SVG and load_svg.")]
    async fn filter_svg(&self, Parameters(FilterSvgRequest { in_path, out_path, rules }): Parameters<FilterSvgRequest>) -> Result<String, ErrorData> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.send(Command::FilterSvg { in_path, out_path, rules, respond: tx });
        let v = rx.await.map_err(|e| to_mcp_err(e.to_string()))?.map_err(to_mcp_err)?;
        Ok(v.to_string())
    }

    #[tool(description = "Render a group to an SVG file on disk, so it can be viewed as an actual image - use this when numeric stats/list_shapes can't resolve a question (e.g. is this really braille, does this look like the right artwork).")]
    async fn render_group(&self, Parameters(RenderGroupRequest { handle, group, out_path }): Parameters<RenderGroupRequest>) -> Result<String, ErrorData> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.send(Command::RenderGroup { handle, group, out_path, respond: tx });
        let v = rx.await.map_err(|e| to_mcp_err(e.to_string()))?.map_err(to_mcp_err)?;
        Ok(v.to_string())
    }

    #[tool(description = "Per-shape centroid/bounding-box/area as plain numbers - e.g. to compute the offset between a shape's original position and where `nest` placed it (nest_translation - centroid, when rotation is disabled).")]
    async fn shape_info(&self, Parameters(ShapeInfoRequest { handle, index }): Parameters<ShapeInfoRequest>) -> Result<String, ErrorData> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.send(Command::ShapeInfo { handle, index, respond: tx });
        let v = rx.await.map_err(|e| to_mcp_err(e.to_string()))?.map_err(to_mcp_err)?;
        Ok(v.to_string())
    }

    #[tool(description = "Translate every shape in a group by (dx, dy) and render the result to an SVG file - e.g. to place a sign's full content at the same spot its nested border profile ended up.")]
    async fn translate_and_render(&self, Parameters(TranslateAndRenderRequest { handle, group, dx, dy, out_path }): Parameters<TranslateAndRenderRequest>) -> Result<String, ErrorData> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.send(Command::TranslateAndRender { handle, group, dx, dy, out_path, respond: tx });
        let v = rx.await.map_err(|e| to_mcp_err(e.to_string()))?.map_err(to_mcp_err)?;
        Ok(v.to_string())
    }

    #[tool(description = "Combine several groups (each with its own dx/dy) into ONE rendered sheet - e.g. several signs' full content, each placed at wherever its own nested border ended up, matching one laser sheet's layout for milling.")]
    async fn translate_groups_and_render(&self, Parameters(TranslateGroupsAndRenderRequest { handle, translations, sheet_borders, out_path }): Parameters<TranslateGroupsAndRenderRequest>) -> Result<String, ErrorData> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.send(Command::TranslateGroupsAndRender { handle, translations, sheet_borders, out_path, respond: tx });
        let v = rx.await.map_err(|e| to_mcp_err(e.to_string()))?.map_err(to_mcp_err)?;
        Ok(v.to_string())
    }

    #[tool(description = "Same as translate_groups_and_render, but keeps each shape's real fill/stroke color instead of the forced black-hairline-outline look - for a 'what does the finished sign actually look like' render (position/rotation correct, real colors) rather than a manufacturing/toolpath layout.")]
    async fn translate_groups_and_render_styled(&self, Parameters(TranslateGroupsAndRenderStyledRequest { handle, translations, sheet_borders, out_path }): Parameters<TranslateGroupsAndRenderStyledRequest>) -> Result<String, ErrorData> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.send(Command::TranslateGroupsAndRenderStyled { handle, translations, sheet_borders, out_path, respond: tx });
        let v = rx.await.map_err(|e| to_mcp_err(e.to_string()))?.map_err(to_mcp_err)?;
        Ok(v.to_string())
    }

    #[tool(description = "Place several groups using jagua-rs's own placement data directly (rotation_degrees + translation from nest's output, plus each group's reference-shape centroid from shape_info as the pivot), rendered with gel's own SVG output - not jagua-rs's colored renderer/theme. Reduces to plain translation when rotation_degrees is 0.")]
    async fn place_groups_and_render(&self, Parameters(PlaceGroupsAndRenderRequest { handle, placements, sheet_borders, out_path }): Parameters<PlaceGroupsAndRenderRequest>) -> Result<String, ErrorData> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.send(Command::PlaceGroupsAndRender { handle, placements, sheet_borders, out_path, respond: tx });
        let v = rx.await.map_err(|e| to_mcp_err(e.to_string()))?.map_err(to_mcp_err)?;
        Ok(v.to_string())
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for GelInspector {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Inspect gel-loadable drawings (SVG, or CDR pre-converted via LibreOffice). \
                Load a file, then filter/group_by/sort/stats/list_shapes over it using the same \
                JS-predicate query pipeline gel's Filter/GroupBy/Sort already implement.",
            )
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let service = GelInspector::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
