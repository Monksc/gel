# gel-mcp Design Overlay (draft, for discussion — nothing below is committed)

Status: scaffold compiles and the full pipeline works end-to-end against
a real production file (`SIGN TYPE C.1 ARTWORK.cdr` → LibreOffice SVG →
`load_svg` → `filter` with `depth(i) % 2 == 0 && depth(i) > 1` → `stats` →
`list_shapes` → the honest `nest` stub). `load_svg` is just slow on real
files (~2 minutes for 1048 shapes, not hung) — see "Known issues" below.
Everything in this doc is the plan for what comes next, per "stop and
write the overlay before making more changes."

## 1. Why this exists

Two separate things, don't conflate them:
- **This crate**: an inspection tool so Claude can query/"feel" a loaded
  drawing interactively, today, while we're still in Sep Wk1's file-ID
  work.
- **AUTOMATION_ROADMAP.md §1.5's planned "MCP server for gel"**: the
  production surface that drives real layout runs, sequenced later,
  after the deterministic layout engine exists. This crate is a
  reasonable head start on that, but the two goals (interactive inspection
  vs. production driving) may end up wanting different APIs — revisit
  once §1.5 actually starts.

**Architecture decision (per discussion, important - shapes what should
and shouldn't become a tool here going forward)**: the actual
metadata/addon-driven layout *rules* (material thickness → offset amount,
conditional instructions, etc. - see §7 below) are meant to live in
**the database**, as data the deterministic pipeline reads directly - they
explicitly do **not** need an MCP server to run. This crate's job stays
narrower: helping Claude understand/inspect a drawing now, and later
*checking over* the rules engine's output as a QA layer - not being the
thing that executes the rules. Don't let tool design here drift into
reimplementing that rules engine.

## 2. Visualization strategy (per discussion)

Two tiers, deliberately not one:

1. **Numeric/statistical layer (cheap, default-on)** — per shape or group:
   count, area, perimeter, bounding box, centroid/position, position
   relative to parent shape, point count, `circle_metrics` (circularity).
   This is the primary way Claude inspects a file — cheap enough to
   return broadly.
2. **Drill-down layer (expensive, opt-in only)** —
   - `shape_points(handle, index)`: full raw point/line list for exactly
     one shape, when the numeric summary can't resolve a question (e.g.
     "is this curve actually closed").
   - `render_group(handle, group, format: svg|png)`: renders a filtered
     group via `gel::save_svg` (extend for PNG via a rasterizer, or just
     shell to the same LibreOffice svg->png path this app already has
     elsewhere) to a temp file, returns the path so Claude can `Read` it
     as an actual image. This is how ambiguous cases (real braille vs.
     incidental dots, artwork vs. notes-page) actually get resolved -
     not by cramming pixel data into every tool response.

Both drill-down tools are called explicitly, per-shape or per-filtered-
group, never dumped automatically for a whole file - that's the cost
control.

## 3. Full tool surface (mapping asgdraw.txt's operation inventory)

Numbering matches asgdraw.txt as given, **except item 9 (Kerning) - that's
not in asgdraw.txt at all**, it came from the separate `ivy`/`gel`
discussion and is appended here for completeness, not because the source
file numbers it that way.

1. **Selection** — `list_shapes`/`stats` on the current group already
   covers "see what's tagged." Auto-tag/predicate-tag is covered by
   Filter/GroupBy. **Gap: "manually tag" is not covered** - asgdraw.txt
   distinguishes auto-tagging from directly assigning a *specific known*
   shape (or set of indices) into a named group, with no predicate
   involved - e.g. after picking a shape out visually via `render_group`
   or via `similar_shapes` (§6.5), there's currently no tool to just say
   "put shape #42 into group X." Needs a `tag(handle, indices, group)`
   tool - trivial once designed, just missing.
2. **Auto-tag**: Text, Symbols, Braille, Bound(ing shapes) — text/braille
   auto-tagging already exists and works well per the roadmap; needs a
   `auto_tag` tool wrapping whatever that existing classifier is (need to
   locate it - likely in `ivy`, not yet in `gel` itself). Symbol/Bound
   auto-tagging status still unconfirmed per roadmap - investigate before
   building.
3. **Filter** — done (`filter` tool, wraps `gel::Filter`).
4. **Transform**: None / Fill (colour, none) / Offset / Outline (colour,
   none, width, solid) / increase width or height / extend left, top,
   right, bottom / convert to basic shapes (circles, rectangles, etc) /
   matrix multiplication. **Not yet checked whether `gel::queries::transformation`
   already covers these** - `transformation.rs` exists in `gel` but
   hasn't been read yet for this doc. Next step before building a
   `transform` tool: read `gel/src/queries/transformation.rs` and map its
   existing fields against this exact list; only add new gel-side
   variants for what's missing, don't reinvent what's there.
5. **Bin packing / nesting** — genuinely missing from `gel` (confirmed,
   no packing crate in its `Cargo.toml`). See §4 for candidate evaluation.
   Needs: width/height/amount per box, sheet priority order,
   machine-specific packing anchor (mill starts bottom-left, laser starts
   top-left), configurable spacing between nested shapes, and **preserve
   orientation** as an explicit constraint (per this discussion) - not
   every nesting algorithm supports "don't rotate this shape," so that's
   a hard filter on which crate to adopt, not a nice-to-have.
6. **Add-in shapes** (e.g. reference/backer boxes) — not yet designed;
   needs a tool to inject a plain rectangle/box into a group at a given
   position, probably trivial once nesting's box/sheet model exists.
7. **Other**:
   - Grid-cut (remove extra/redundant lines) — not yet located in `gel`;
     needs investigation.
   - Invert — meaning still unclear (per the roadmap's own "wish I
     remembered what this meant" note) - needs Cameron to clarify before
     it can be scoped at all.
   - Font to curves — this happens upstream, in the CDR/LibreOffice
     export step (already confirmed: no raw `<circle>`/`<ellipse>`
     elements survive that export, everything's paths) - likely nothing
     to build here, just confirm gel's SVG import doesn't need to redo it.
   - Preserve orientation — folded into §5's nesting requirement above.
8. **Replication** — asgdraw.txt's "line to line" refers to CSV rows
   (per Cameron - asgdraw.txt itself didn't describe it well), i.e. it's
   the same thing as the CSV-driven replication below, not a separate
   drawing-consistency feature. Two modes, not three:
   - **Braille to braille** — re-translation (not visual copy) via `ivy`'s
     existing `translate_braille_c` FFI wrapper
     (`ivy/src/braille/mod.rs::translate`), same reuse path
     AUTOMATION_ROADMAP.md §0.5 already calls out.
   - **Symbol** replication, scoped here as **CSV-driven replication**
     (per this discussion, not from asgdraw.txt itself): a CSV whose
     header row is the original template text and whose remaining rows
     are per-sign replacement values; each row expands into one
     replicated sign with that row's text substituted in - braille cells
     within it go through the braille-to-braille path above rather than a
     plain copy. Needs a `replicate_from_csv` tool; exact CSV/column
     contract still to be designed with Cameron.
9. **Kerning** — `gel::Kerning` already has real settings
   (`set_inner_shapes`/`get_inner_shapes`, `borders_group`, `epsilon`,
   `space`, `respect_space`, and Left/Top/Right/Bottom/Center direction
   handling in `kerning.rs`) - further along than this doc assumed before
   reading it. Needs: (a) a `kern` MCP tool wrapping it, (b) confirming
   `respect_space` already covers "preserve word spacing vs. correct to
   exact letter spacing vs. given exact spacing" from this discussion, or
   whether that needs a new field, (c) checking how `ivy` actually drives
   this today (`ivy/src/gelhelp/mod.rs`'s `MyQueries::Kerning`) for real
   default values worth carrying over rather than guessing new ones.

## 4. Nesting crate candidates (for discussion, not decided)

All three real irregular-nesting candidates found on crates.io are young
- this is a maturity risk across the board, not just one crate:

| Crate | Approach | Version | Repo age / activity | Risk notes |
|---|---|---|---|---|
| `u-nesting-d2` (+ `u-nesting-core`, part of the `u-nesting` family) | GA/SA/ALNS metaheuristics + true No-Fit-Polygon collision detection, C FFI included | 0.7.2 | created Jan 2026, pushed Jul 2026, 12 stars, 0 open issues | Most feature-complete for what we need (real NFP-based irregular nesting), but very new and effectively single-maintainer (`iyulab`) - "0 open issues" on a 6-month-old repo reads as low external usage, not necessarily high quality. |
| `nfp` | Just the No-Fit-Polygon primitive itself, no packer on top | 0.3.3 | created Nov 2025, pushed Nov 2025, 5 stars | Would mean building our own packing/placement loop on top - more control, more work. Even younger/smaller than u-nesting. |
| `bin-packing` | Rectangular/cut-list 1D/2D/3D bin packing | 0.3.0 | not yet checked | Wrong shape of problem - rectangular only, not true irregular polygon nesting. Only relevant if we decide rectangular nesting is good enough for some machines (roadmap doesn't currently assume that). |
| `kaosu-packer` | Biased random-key genetic algorithm, 2D/3D bin packing | 0.1.0 | not yet checked | Unclear if it handles irregular polygons or just rectangles/boxes - needs reading before it's a real candidate. |
| `binpack-3d`, `pack_it_up` | 3D box-fitting | 0.1.0 / 1.1.0 | not yet checked | Wrong problem shape (3D boxes-in-box, not 2D irregular nesting) - listed only because they showed up in the search, not real candidates. |

**Update - a much stronger candidate found**: `jagua-rs` +
`sparrow` (both github.com/JeroenGar). Very different maturity tier from
everything above:

| Crate | Stars / forks | Age / activity | License | Notes |
|---|---|---|---|---|
| `jagua-rs` | 186 / 51 | created Jan 2024, pushed Jun 2026 | MPL-2.0 | Collision-detection *engine* for 2D nesting (bin packing, strip packing, multi-strip) - quadtree + fail-fast surrogates + hazard proximity grids, not NFP. Peer-reviewed (INFORMS Journal on Computing, arXiv preprint), funded academic research (KU Leuven CODeS group), CI with tracked perf benchmarks. Per-item `allowed_rotation: RotationRange` enum with `None` / `Continuous` / `Discrete(Vec<f32>)` variants - `RotationRange::None` is exactly the "preserve orientation" hard constraint this discussion flagged as a must-have filter on which library to adopt, built in as a first-class option, not a workaround. |
| `sparrow` | 331 / ? | created Nov 2024, **pushed 3 days ago (Aug 18 2026)** | MIT | The actual optimizer/solver built on `jagua-rs` ("state-of-the-art... for 2D irregular strip packing" per its own description) - `jagua-rs` explicitly says use this for production, its own bundled `lbf` crate is just a reference implementation. |

**Adjacent finds worth tracking (not nesting itself, don't lose these)**:
- `u-nesting-cutting` (same `iyulab/U-Nesting` family) - **post-nesting
  cutting-path optimization**: given already-placed/nested parts, computes
  an optimized cut sequence (pierce points, travel distance, TSP-based
  ordering with precedence constraints). This is a §2.5 Automatic Tool
  Pathing candidate, not a nesting candidate - relevant later, not now.
- `u-schedule` (`iyulab/u-schedule`, separate repo, same author) - a
  job-shop scheduling library (dispatch rules, GA, constraint programming)
  - a §2.6 Automatic Scheduler candidate, not relevant to this crate at
  all. Noted here only so it isn't lost before that phase starts.

This is a different tier of maturity than the `u-nesting`/`nfp` family
(10-100x the stars/forks, actively maintained as of days ago, academic
backing with published benchmarks vs. a few-months-old single-maintainer
repo). Strip packing (not just bin packing) also matches this doc's §5
box/sheet framing better than pure rectangular bin-packing would.

**Hands-on eval done (2026-08-24)** - cloned `jagua-rs`, built its `lbf`
reference CLI (needed a `rustup update stable` - the crate requires
rustc 1.90, this machine had 1.88 - done, isolated to this eval checkout,
not `gel`/`ivy`'s toolchain), ran it for real against its own bundled
`swim.json` benchmark instance (10 distinct irregular polygon shapes,
demand 3 each = 48 items) and a hand-authored polygon-with-holes case:

- **Real nesting works**: 48 genuinely irregular polygons packed at
  60.1% density on a strip, valid JSON + SVG output, visually inspected
  the SVG - correct, non-overlapping placements.
- **`RotationRange::None` verified, not just read from source**: forced
  every item's `allowed_orientations` to `[0]` only, re-ran - output
  showed literally one distinct rotation value (`0.0`) across all 48
  placements, and density dropped to 57.7% (the honest physical cost of
  disallowing rotation, not a bug). The no-rotation/preserve-orientation
  constraint this discussion flagged as a hard requirement genuinely
  works as advertised.
- **Strip-packing maps cleanly onto §1.5's packing policy**: fixed
  `strip_height`, solver finds the minimal `strip_width` needed - "how
  wide a sheet do I actually need" is directly the same question as "does
  it fit the smallest box," just continuous instead of picking from a
  discrete box list. Combining with a discrete box-size choice afterward
  (round the found width up to the smallest available real sheet size,
  or route to the biggest sheet if it doesn't fit any) is straightforward
  glue code, not a gap in the crate.
- **Fixed max width/height (a real sheet cap, not open-ended) is a
  different problem mode, not a config knob on strip packing**: jagua-rs
  also has `bpp` (bin packing), where you give it one or more named bin
  *types* (`ExtBin`: a shape/size, a `stock` count of how many physical
  sheets of that size exist, and a `cost`) and it packs across as many
  bin instances as needed. Verified for real: built a second example
  (`~/Projects/rust/eval/nest_example/src/bin/max_width.rs`) capping the
  same 16-item set from the strip-packing example at a 100x100 sheet
  (deliberately too small for all of them at once) with
  `stock: usize::MAX` - it automatically opened **4 sheets** (4 items
  each) rather than failing or overflowing the bound. `stock`+`cost` on
  multiple bin types is exactly the mechanism for §1.5's resolved policy
  ("try the smallest box everything fits in first, else the biggest box,
  sheet priority in order") - give it a small preferred sheet type and a
  large fallback type and let it choose across them. **Not yet verified**:
  whether `cost` actually drives a preference ordering when multiple bin
  types are offered together (only tested one bin type at a time) -
  flagged, not blocking, Cameron confirmed this is enough to settle the
  question of whether a max-size constraint exists at all.
- **Real limitation found, not assumed**: **polygon holes are NOT
  supported yet** - tested directly with a hand-authored
  square-with-square-hole item, got an explicit runtime warning: `No
  native support for polygons yet, ignoring the holes`. It still nests
  the item (treating it as solid, hole silently dropped), so this isn't
  a crash/blocker, just lost fidelity: a small part could theoretically
  nest inside that hole and this won't ever find that placement.
  **Likely a non-issue in practice** though - per AUTOMATION_ROADMAP.md
  §1.5, nesting operates on each *sign's outer profile* (e.g. a backer
  panel's outline), not on individual hole-bearing letterforms within a
  sign - and treating a hole as solid is conservative/safe (never causes
  real material overlap), it just misses a micro-optimization. Worth
  confirming this framing is actually right (does any real
  `SIGN TYPE` file need a hole-aware *outer* profile nested, not just
  hole-bearing letters inside one) before fully discounting the gap.

**Recommendation**: `jagua-rs`/`sparrow` remains the strongest candidate
- maturity, verified rotation constraint, and the one real gap found
(holes) likely doesn't bite for how nesting is actually scoped. Still a
joint decision, not picked here - but this is no longer "read the README
and hope," it's a verified hands-on result.

## 5. Known issues (blocking further tool-building until understood)

- **FIXED**: `Data::from_respect_indexes` was slow on real files (~2
  minutes for 1048 shapes, ~2206 shapes untested before the fix) because
  `depth_tree::Tree::add_node_tree_node` recomputed `unsigned_area()` and
  `interior_point()` from raw polygon geometry on every single containment
  comparison during tree build, instead of using the `area`/`center_point`
  `TreeNode` already caches at construction. Root-caused and fixed in
  `depth_tree/src/tree.rs`: added `Shape::exact_area()`/
  `exact_interior_point()`/`contains_interior_point()`, cached as f64 on
  `TreeNode` (not the existing f32 `area`/`center_point` fields - those
  are for the spatial index and truncating them for this check silently
  changed real output, caught via a depth-histogram diff before landing).
  1048 shapes: ~2min → ~1.6s. 2206 shapes: previously never finished in
  any test window → ~4.3s. Verified byte-identical `depth_histogram`,
  `filter` match count, and `total_area` (to the last float digit) against
  the pre-fix baseline on the same file - this is a pure memoization, not
  a behavior change.
  - **Not yet shipped**: the fix lives in the local `~/Projects/rust/depth_tree`
    checkout, and `gel`'s `Cargo.toml` has a *temporary* path-dependency
    override pointing at it (marked as such, with the original pinned git
    dep commented out alongside). To actually ship this: commit + push
    `depth_tree`, then bump the pinned `rev` in both `gel`'s and `ivy`'s
    `Cargo.toml` to the new commit, then revert `gel`'s override. Not done
    yet - needs a deliberate decision (this touches a dependency shared
    with `ivy`, production software), not something to push silently.
  - A pre-existing, unrelated test failure was found while checking this
    (`depth_tree`'s only unit test, `tree::tests::it_works`, panics on a
    hardcoded `/home/cameron/Downloads/CAM.svg` path) - confirmed via
    `git stash` that it fails identically before this fix too, so it's not
    a regression, just a pre-existing gap (worth fixing separately, not
    address here).
- Already fixed as part of this scaffold: `gel`'s several `println!`s
  (data.rs, filter.rs, groupby.rs, loop_over.rs) were writing to stdout,
  which corrupts the MCP stdio transport's JSON-RPC stream and hangs the
  client. Changed to `eprintln!` in `gel` directly - needed for any
  stdio-based MCP server around `gel`, not specific to this crate.

## 6. Similarity search ("find shapes like this one")

New requirement (per discussion): given one shape, rank every other shape
in the file by how similar it is - for batch operations like "recolor
this symbol everywhere it appears" or "remove this decorative element
from every sign" without hand-picking each instance.

**Feature vector per shape** (all already cheap via the numeric layer in
§2, or one geo op away):
- size: area, perimeter, bbox width/height, aspect ratio
- shape complexity: point count, `circle_metrics(i).circle` (circularity)
- structural: depth, child count (once available - see §3 item 2)
- position: **must be sign-relative, not absolute file coordinates** -
  "similar position" across two different signs only means anything if
  each position is expressed relative to that shape's own sign's bbox
  (e.g. "20% in from the left edge, 10% down"), not raw SVG coordinates.
  This needs a "which sign does this shape belong to" concept that
  doesn't exist yet - working assumption to verify against real data:
  the ancestor at some fixed shallow depth (depth 1? depends on the
  file - c1.svg vs e.svg had different depth distributions per earlier
  testing) is one sign, everything nested under it is that sign's
  contents. Don't hardcode this without checking it against a few real
  files first, per this doc's own "verify before automating" pattern.

**Shape/overlay similarity** (the strongest signal per this discussion -
"if we can define anything else, overlay will be very close"): translate
both candidate polygons so their centroids coincide (or align by
bounding-box corner - try both, compare), then compute intersection-over-
union via `geo-clipper` (already a `gel` dependency, unused so far) -
`intersection_area / union_area`, 1.0 = identical shape, 0.0 = no overlap
at all after alignment. This is a much stronger "is this actually the
same symbol" signal than comparing scalar features alone, which can
false-positive on coincidentally-similar area/perimeter for genuinely
different shapes.

**Indexing approach (per discussion): an actual `rstar::RTree`, not brute
force.** `rstar::Point` is generic over any fixed-size array
(`impl<S, const N: usize> Point for [S; N]` - confirmed in rstar 0.12.2's
source, already `gel`'s pinned version), so this doesn't need a new
crate or a bespoke distance search - it's the same technique `gel`'s
`Kerning` already uses for spatial queries (`kerning.rs`'s `Node<T>`
wrapping a point for `RTreeObject`/`PointDistance`), just applied to a
feature-space point instead of an XY point:
- Build an `[f32; N]` vector per shape from **caller-specified features**
  (a name list, e.g. `["area", "perimeter", "aspect_ratio", "depth",
  "sign_relative_x", "sign_relative_y"]`), each entry pre-scaled by a
  caller-specified weight before insertion - scaling a dimension by `w`
  scales its contribution to rstar's squared-distance metric by `w²`,
  which is the standard way to make nearest-neighbor weighting work
  without a custom distance function.
- Insert every shape's vector into an `RTree<FeaturePoint>` (wrapper
  struct, same shape as `Kerning`'s `Node<T>`), then answer `similar_shapes`
  via rstar's `nearest_neighbor_iter` from the query shape's own vector -
  O(log n) per query instead of scanning every shape.
- **Overlay IoU doesn't fit cleanly as an index dimension** (it's a
  pairwise polygon operation, not an independent per-shape scalar), so it
  stays a second pass: use the rstar query to cheaply narrow to a
  candidate shortlist (e.g. top 20-50 by feature distance), then only
  compute the expensive `geo-clipper` intersection-over-union on that
  short list to re-rank/annotate the final `top_k` - cheap filter, then
  expensive exact check only where it still matters, same pattern as the
  bounding-box-then-exact-check idea explored (and reverted) for the
  `depth_tree` fix above.

**Proposed tool**: `similar_shapes(handle, group, index, top_k, features, weights?)`
- `features`: which named dimensions to index on for this call (caller
  picks - "things we specify," not a fixed baked-in vector).
- Returns top_k matches **with the per-feature breakdown and the overlay
  IoU score**, not just a combined number - so it's possible to see *why*
  something matched.
- Once a similar-shapes group is found, it should feed straight into the
  existing `filter`/future `transform` tools (§3 item 4) for the actual
  batch recolor/remove - this tool's job is finding the group, not acting
  on it.

Not built yet - flagging the sign-relative-position dependency and this
rstar indexing approach for discussion before implementing.

## 7. Layout settings/rules engine (tracked here, not designed here)

Separate pending thread, deliberately not designed in this file since
it's DB-shaped, not `gel-mcp`-shaped (see §1's architecture decision):
metadata+addon-driven rule fields like `{location: "meta", key:
"thickness"}` / `{location: "sign", key: "width"}` /
`{location: "addon", regex: ...}` piped through an eval step to get exact
numbers, then conditional instructions (`if thickness > 0.25 then ...`).
Source data for these rules: `The (close to) Ultimate Guide to making
layouts for the Laser.docx` (now in the `asg-quote-cost` base dir) has
the real per-material numbers this needs - offsets by thickness (1/32"
→0.004", 1/16"-1/4" →0.008", mylar →none), max piece sizes per material,
radial-edge/grid-line spacing (1/8"), process sequencing (paint/print
order depends on subsurface vs. surface). Needs its own design pass with
Cameron (schema, where in `asg-quote-cost` it lives) - not started.

## 8. What's already scaffolded vs. not

**Scaffolded and building successfully**: `load_svg`, `filter`, `group_by`,
`sort`, `stats`, `list_shapes`, `nest` (explicit not-implemented stub).

**Not started, pending this doc's discussion**: `transform`, `auto_tag`,
`tag` (manual/direct group assignment, MCP-only - never a production op
per Cameron - §3 item 1), `shape_points`, `render_group`,
`replicate_from_csv`, `kern`, `similar_shapes` (§6), add-in shapes,
grid-cut, invert. Intentionally not built yet - this doc is the
checkpoint before building them.
