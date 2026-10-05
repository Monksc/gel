# CLAUDE.md

## Working With Claude

- **When Cameron asks a question, answer it — don't also make changes** unless he separately asks for the change. A question is a request for information, not implicit approval to act on the answer.

## What This Is

An MCP server that lets an LLM (Claude) interactively inspect and query
sign-shop artwork drawings by wrapping the parent `gel` crate's existing shape
query/transform pipeline. It exists to solve the "let Claude feel the
.cdr files" step of `asg-quote-cost/AUTOMATION_ROADMAP.md`'s Sep Wk1 work
(file identification: which uploaded file is the sign-type artwork).

It is **not** the same thing as the "MCP server for `gel`" planned in that
roadmap's §1.5 for driving production layout runs later. This one is for
interactive inspection/design work now; that one is a production
automation surface, sequenced after the deterministic layout engine
exists. Keep the two separate — don't let this scaffold quietly become
the production surface without a deliberate decision to merge them.

## Architecture

- `gel` (path dependency, `..` - this crate is a workspace member inside
  the `gel` repo itself, see its root Cargo.toml) does all the real work: shape data,
  JS-predicate queries (`Filter`/`GroupBy`/`Sort`/`Kerning`), SVG I/O.
- `boa_engine::Context` (gel's embedded JS engine) is **not `Send`**, so
  every loaded `gel::Data` lives and dies on one dedicated OS thread
  (`src/worker.rs`'s `WorkerHandle`), never touched from the async
  runtime directly. Tool handlers send a `Command` over a `std::sync::mpsc`
  channel and `.await` a `tokio::sync::oneshot` reply. Don't try to make
  `Data` cross threads — restructure around the worker instead.
- `src/main.rs` is the `rmcp` MCP server (stdio transport) — one
  `#[tool]`-decorated method per gel operation, using `rmcp`'s
  `tool_router`/`tool_handler` macros.
- `src/nest.rs` is a deliberate placeholder — no nesting/bin-packing crate
  has been evaluated and wired in yet. See `DESIGN.md` for candidates.

## Critical gotcha: stdout is the MCP transport

The stdio MCP transport uses this process's own stdout for the JSON-RPC
stream. `gel` has several debug `println!`s (data.rs, filter.rs,
groupby.rs, loop_over.rs) — those **corrupt the protocol stream** if left
as `println!`, causing the client to hang waiting for a reply that never
parses. They were changed to `eprintln!` in `gel` itself as part of this
scaffold (see gel's git history) specifically to make this workable. If
`gel` gains new debug output, it must go to stderr, never stdout, or any
future MCP server built on it breaks the same way.

## Known issue: `gel::Data::from_respect_indexes` was slow on real files (FIXED)

Loading a real `SIGN TYPE *.cdr`-derived SVG used to take ~2 minutes for
~1000 shapes, pegging a CPU core in `depth_tree::Tree::from_polygon_id`.
Root-caused: `add_node_tree_node` recomputed `unsigned_area()`/
`interior_point()` from raw polygon geometry on every containment
comparison instead of caching them. Fixed in `depth_tree/src/tree.rs`
(new `Shape::exact_area()`/`exact_interior_point()`/
`contains_interior_point()`, cached as f64 on `TreeNode`) — verified
byte-identical tree output vs. the pre-fix baseline. ~1000 shapes now
loads in ~1.6s, ~2200 shapes in ~4.3s. See `gel-mcp/DESIGN.md` §5 for
full detail.

**Not yet shipped**: `gel/Cargo.toml` currently has a *temporary*
path-dependency override pointing at the local `depth_tree` checkout
(marked as such in a comment, with the original pinned git rev commented
out alongside it). To ship for real: commit + push `depth_tree`, bump
the pinned `rev` in both `gel`'s and `ivy`'s `Cargo.toml`, then revert
`gel`'s override back to the git dependency. Deliberately not done yet —
this touches a dependency `ivy` (production software) also pins.

## Conventions

- Every new tool should map to an existing `gel`/`asgdraw.txt` operation,
  not invent new geometry logic here — this crate is a thin MCP shell
  over `gel`, matching the roadmap's "extend an existing engine" framing.
- Read-only by default. A tool that mutates a loaded `Data` in place
  (rather than writing to a new/named group) needs a deliberate reason.
