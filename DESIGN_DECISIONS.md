# gel — design decisions and gotchas

Why the instruction set looks the way it does, and the things in this codebase
that will bite you if nobody warned you.

Lives in the gel repo rather than with the layout plan because it is about
*this code*. The ISA roadmap (phases, open questions) is in
`asg-quote-cost/CLAUDE-CODE/INSTRUCTION_SET_PLAN.md`.

---

## 1. The model

**Named groups are the registers.** Every instruction is `get_group` →
`set_group`. A group is `Vec<Vec<usize>>` — a list of **slots**, each slot a
list of shape indices. Slots are meaningful: `Nest` writes one slot per sheet,
`Filter` keeps or drops whole slots.

**Shapes are append-only.** `write_result_slot` never mutates a shape in
place; it appends new ones and points a slot at them. So `Copy` really is a
copy, and an earlier group still sees the geometry it saw before.

**Instructions are externally-tagged serde enums** — `{"Filter": {...}}` —
because that is the shape ivy's existing saved settings already use. Do not
change the tagging; it would break every stored program.

**Everything numeric is an expression string**, evaluated by Boa against a
shared JS context. `"0.004"`, `"kerf"`, `"n * (sheet + gap)"` are all valid.
`RunCode` is how a program puts values into that context.

---

## 2. Decisions, and why

### `SetData` instead of `Fill` / `Outline`

Paint used to be four typed vectors (`fill_rgba`, `stroke_rgba`,
`stroke_width`, `visible`) plus two instructions. Adding "stroke dash" would
have meant a fifth vector, a third instruction and another `Emit` branch — and
milling wants tool number, feed rate and depth while 3-D print wants
orientation and supports, none of which are paint.

Now there is one `shape_data: Vec<serde_json::Map<String, Value>>`.

* `Emit` interprets `fill`, `stroke`, `stroke_width`, `visible`.
* **Every other key is written out as a `data-*` attribute**, so a milling
  tool number survives into the file for a downstream step without `Emit`
  knowing what it means.
* `SetData` **merges** rather than replaces, so setting `fill` does not drop a
  `stroke` set earlier. `null` removes a key; `replace: true` clears first.

### `Define` / `Call`, and deliberately no `Include`

Boa already gave reusable *expressions*. These give reusable *instruction
sequences*.

There is **no file loading and no `Include`**. The quote generator stores the
shared library of `Define`s and each layout program separately and
concatenates them before upload. So the server runs one self-contained file:
no filesystem access, no path resolution, and the composed program is an exact
re-runnable record of the job. Because `Define` is a plain instruction,
composition is array concatenation — `jq -s 'add' library.json layout.json`,
or `[...library, ...layout]` in Node.

**No return values.** Groups are the registers, so a function writes to
whichever group the caller names — an out-parameter. Real returns would mean
inventing anonymous temp groups and a naming scheme for them.

**Argument typing has exactly one rule.** A string that is *entirely* one
`{expression}` binds with its type intact (`"{stock}"` → the number 30); any
other string is a template producing a string (`"sheet-{n}"` → `"sheet-0"`);
non-strings bind as-is. Without this, `"{kerf}"` would arrive as `"0.125"` and
`Nest` would reject it.

**`__call` gives scratch groups.** A body writes `"tmp-{__call}"` and each
call site gets its own — locals without building a scope system.

**Redefining a name is an error**, not a silent override. The composed program
is machine-generated, so a collision is a composition bug the UI should catch
at save time.

### Templates in group names

`"sheet-{n}"` expands; `"panels"` does not. A bare string cannot be evaluated
as code because `panels` *is* a valid group name — it would have to be quoted,
breaking every existing program. `{{` and `}}` are literal braces. **A lone
`}` is an error**, so a mistyped template fails loudly instead of naming the
wrong group.

### Annotations are not shapes

`AddText` records into a flat `Vec<Annotation>`; `Emit` writes them as
`<text>` only when asked. A label is never cut, so making it geometry would
mean carrying glyph outlines — a font engine — for something the SVG viewer
renders for free. One program over one set of geometry therefore produces both
the cut file and the annotated "verbose" copy.

### `Nest` is feature-gated

Behind `--features nest`, because ivy should not compile a nesting engine it
does not use. Without the feature a program using `Nest` **fails to parse**
("unknown variant") rather than running and quietly producing an unnested
layout.

`Nest` deliberately omits `origin` and `allow_mirror`: `jagua-rs` exposes only
`allowed_orientations`, and `lbf` packs from its own origin, so both would be
accept-and-ignore. **Laser packs top-left and milling bottom-left**, so
`origin` will have to be a post-transform when milling lands.

`lbf` ("Left-Bottom-Fill") is the reference optimizer shipped with `jagua-rs`
— a greedy heuristic that places each part at the lowest-then-leftmost
feasible spot and never backtracks. It is seeded (`SmallRng::seed_from_u64(42)`)
so runs are reproducible. It is not optimal: 13 panels that fit one 30×30
sheet came out 12 + 1.

### Colours

Validated at emit time, not when set, so `SetData` stays generic. Validation
exists because **SVG renders an unrecognised colour as black** — a typo would
silently produce a wrong proof rather than an error.

---

## 3. Gotchas — read this before you debug anything

### `Data::from` REORDERS shapes

Building the containment tree renumbers them; `from_respect_indexes` returns
the mapping. **Never address a shape by a raw index in a program or a test.**
Select it by geometry. This has already produced two wrong tests that looked
like instruction bugs.

### A bare integer means a *slot*, inside an iteration

`Filter` binds `i`, `Sort` binds `l`/`r`, `GroupBy` binds `i`/`j` — all **slot
ordinals**. But `area(i)` / `depth(i)` / `get_data(i, k)` historically meant a
**shape index**. Those agree only when a group's slots are `[[0],[1],[2]...]`.

`main` is identity, so it worked there and **hid the bug for a long time**.
Any group derived from a `Filter` has non-identity indices, so the bare form
read an unrelated shape — silently. On real artwork a chained filter scored 13
where the correct answer was 39.

Fixed 2026-09-21: `Data.current_group` is set by `Filter`/`Sort`/`GroupBy`
(saved and restored, since these nest), and the eleven index-taking globals
resolve through `bare_index_targets()`. Outside an iteration there is no group
in scope and a bare integer is still a shape index.

Per-global slot semantics: `area` and `len` **sum** over the slot, `frame` is
the bounding rect of the whole slot, and `depth` / `fill_color` /
`stroke_color` / `is_visible` / `get_data` / `center` / `circle_metrics` /
`distance` answer from the **first** shape.

**Residual:** `GroupBy` binds `i` to a slot of `get_group` but `j` to a slot
of the accumulating `set_group`, and only one group can be current. `i` is
correct; address `j` explicitly as `area(set_group_name, j)`.

**When adding a global that takes an index**, route it through
`bare_index_targets()` — and note that `center`, `circle_metrics` and
`distance` do it via the shared `get_polygons()` helper, which is a second
code path that had the same bug independently.

### Miscalled globals return `0.0` instead of erroring

`area`, `len`, `frame` and `group_index` all end in `_ => 0.0`. So
`group_index('big', i)` — which needs **three** arguments — silently returns
`0`, and `area(group_index('big', i))` measures shape 0 every time. A typo
gets you a plausible number, not a failure. **Not yet fixed.**

### Templates expand in *name* fields, never in *code* fields

`{expression}` expands in group names, `Emit.out` and `SetData` string values.
It does **not** expand in `Filter.code`, `Sort.compare`, `If.condition`,
`While.condition` or `RunCode.code` — those are raw JS.

Inside a `Define`d function the parameters are already bound as JS variables,
so write the bare name:

```json
"condition": "get_data(src, 0, 'edges') == 'radial'"     ok
"condition": "get_data('{src}', 0, 'edges') == 'radial'" WRONG
```

The wrong form looks up a group literally named `{src}`, finds nothing,
returns `undefined`, and quietly takes the `else` branch. Cost real debugging
time the first time it was written.

### Geometry ops are slot-level; selection and data ops are group-level

`Offset`, `Union`, `Intersect`, `Difference`, `Copy` all default to
`get_index: 0` and act on **one slot**. `Filter`, `Sort`, `SetData` and
`SetOp` act on the **whole group**.

So applying an offset to every part in a multi-slot group needs an explicit
`LoopOver`. Writing `{"Offset": {"get_group": "parts", "set_group": "cut"}}`
against a 39-slot group silently processes one slot and produces a one-slot
result. This is the ISA's sharpest remaining ergonomic edge.

### `write_result_slot` does not touch the style vectors

It extends `shapes` and `depths` only. `Emit` reads shape data with `.get(i)`,
so a short vector is not a panic — it is silence: every derived shape emitted
unstyled. `Data::sync_style_lengths()` is called from `Instruction::query`
after **every** instruction so a new op cannot reintroduce this.

### gel's colour string is not CSS

`fill_color(i)` returns `rgba(r,g,b,a)` with a **0-255** alpha; CSS alpha is
0-1. `color_distance` parses gel's form. `Emit` converts on the way out via
`css_color_string`. Keep the internal form — existing programs compare against
it.

### `geo_clipper` quantises to 1/32768

`CLIPPER_FACTOR = 32768.0`. An offset of 0.1 comes back as 0.100006. That is
0.00003in, far below a thou, but **results are not exact** and a test
asserting equality to 1e-6 will fail.

### Erode-then-dilate silently deletes small parts

The idiom for rounding corners (see §4) removes anything narrower than `2r`
**entirely**, and it does not error — the slot is simply empty. Verified: a
4×1in part with `r = 0.63` vanished while its 6×2in neighbour survived.

### JS globals are flat

`Call` saves and restores each parameter name around the body, or a nested
call clobbers its caller's arguments on the way back out. Recursion is capped
at 64.

### `visible: false` now skips the shape entirely in `Emit`

An invisible shape should not become a cut line. This is a **behaviour change**
from before the `SetData` work — nothing in the source SVG currently sets it,
so no output moved, but be aware.

---

## 4. `CornerRadius` is not a primitive

The laser guide asks for sliders in two variants, "one with radial corners
0.63", one with 90° corners". Rounding convex corners is morphological
*opening*, and `Offset` already has a round join:

```json
{"Offset": {"get_group": "part",   "set_group": "tmp",     "amount": "-r", "join": "round"}},
{"Offset": {"get_group": "tmp",    "set_group": "rounded", "amount": "r",  "join": "round"}}
```

Verified exact on a 6×2in rectangle with `r = 0.63`: the result's area is
`12 − (4 − π)r²` to within 0.01, and the outline goes from 5 vertices to 229.

Three things this does **not** do, and they matter:

1. **Small parts vanish**, silently — see above.
2. **Only convex corners round.** Opening leaves concave corners sharp. Fine
   for a rectangular slider; not a general fillet.
3. **It removes thin features** narrower than `2r` as a side effect.

**Observed for real**, not hypothetically: a program that rounded a 4x1in and
a 6x2in part lost the small one, then failed three instructions later at
`Nest` with *"group 'cut' has no shapes"* — pointing at entirely the wrong
place. This is the strongest argument for building `Assert`.

**Downstream coupling nobody should miss:** the guide says *"Radial edges must
have 1/8" spacing in between"*, while straight edges may share a cut line. So
rounding a part's corners **changes its nesting requirement**. Whatever wraps
this idiom should `SetData` the result (e.g. `{"edges": "radial"}`) and `Nest`
should read it for spacing.
