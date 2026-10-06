# gel

**G**eometrical **E**ngine **L**ibrary — a small instruction set for producing
production layouts from vector drawings.

You give gel an SVG and a program. The program is a JSON array of instructions
that select, measure, reshape, nest and emit geometry; gel runs it and writes
SVG files. There is no scripting host to embed and no application to drive —
the program *is* the configuration, so a layout rule can be stored in a
database, edited in a form, and run unattended.

```bash
gel-cli run programs/all_layouts.json drawing.svg
```

## The model

**Named groups are the registers.** Every instruction reads one group and
writes another:

```json
{"Filter": {"get_group": "main", "set_group": "panels", "code": "area(i) > 1"}},
{"Offset": {"get_group": "panels", "set_group": "cut", "amount": "kerf"}},
{"Emit":   {"get_group": "cut", "out": "output/laser.svg"}}
```

A group is a list of **slots**, each holding shape indices. Slots carry
meaning: `Nest` writes one per sheet, `Filter` keeps or drops whole slots.
`main` is everything loaded from the SVG.

**Shapes are append-only.** Nothing is mutated in place — a derived shape is a
new shape — so an earlier group always still sees what it saw.

**Values are expressions, not constants.** Any numeric field is JavaScript,
evaluated against a context the program itself populates:

```json
{"RunCode": {"code": "thou = 0.001; kerf = 4 * thou; stock = 30;"}},
{"Nest": {"get_group": "cut", "set_group": "sheets",
          "sheet_width": "stock", "sheet_height": "stock", "spacing": "0.125"}}
```

Group names are templates too, so a loop can write somewhere different each
pass — `"sheet-{n}"`, `"output/plate{n + 1}.svg"`.

## Instructions

| | |
|---|---|
| **select** | `Filter` `Sort` `GroupBy` `SetOp` |
| **geometry** | `Offset` `Union` `Intersect` `Difference` `Flatten` `Copy` `Transformation` `Kerning` |
| **control** | `LoopOver` `If` `While` `Define` `Call` `RunCode` |
| **data** | `SetData` `AddShape` `AddText` |
| **output** | `Nest` `Emit` `Assert` |

`Define`/`Call` give reusable instruction sequences; composition is plain array
concatenation, so a shared library and a per-job program are joined with
`jq -s 'add' library.json job.json`.

`Assert` is the one failure that need not be fatal — it halts in debug mode and
collects a warning otherwise, because a layout with one questionable part is
still worth cutting.

## Expressions

Available inside any `code`, `condition` or numeric field:

```
area  center  frame  depth  len  shape_count  distance  group_index
fill_color  stroke_color  is_visible  get_data  color_distance
circle_metrics  offset  union  intersect  difference  flatten
```

A bare integer means a **slot of the group being iterated** inside
`Filter`/`Sort`/`GroupBy`, and a shape index outside one. The explicit forms —
`area('panels', i)` — always work.

## Crates

| | |
|---|---|
| `gel-lib` | the library. Published as the `gel` crate |
| `gel-cli` | `gel-cli run <program.json> <in.svg>` |
| `gel-mcp` | an MCP server for inspecting a drawing interactively |

`programs/` holds runnable examples, including `phase5/full.json` — a complete
job that classifies 13 signs into milled and printed parts, nests them onto
stock, and emits three layouts.

## Notes

`GEL_PROFILE=1` reports per-instruction counts and time. `GEL_FONT_DIR` loads
fonts from a directory instead of the system, which is what a container needs —
text is outlined at import, so a missing font is wrong geometry rather than an
error.

A round `Offset` requires an `arc_tolerance` in drawing units, with no default.
Set it once per program (`arc_tolerance = 0.1 * thou`) and every offset
inherits it. The deliberate absence of a default is explained, with the rest of
the reasoning behind this design, in [`DESIGN_DECISIONS.md`](DESIGN_DECISIONS.md)
— worth reading before changing anything, as it records several things that
look like bugs and are not, and one or two that look fine and are not.
