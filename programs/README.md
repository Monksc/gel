# Layout programs

Runnable gel programs. These are the real artifact the pipeline consumes —
what the quote generator will store per layout and hand to AWS.

    gel-cli run programs/all_layouts.json <artwork>.svg

Output paths are relative to the working directory, so run from the repo root
and files land in `output/`.

## Shape of a program

A program is a bare JSON array of instructions, which is what ivy's saved
settings already are. Job metadata is the **first instruction** — a `RunCode`
that defines `job_name`, `stock`, `kerf` and the rest as JS variables. In
production AWS supplies that block; the rest of the program is unchanged.

There is no `meta_data()` accessor and no `Include`: anything shared is
prepended as plain instructions, so what runs is always one self-contained
file that can be logged and re-run exactly.

## Files

| | |
|---|---|
| `all_layouts.json` | one settings file emitting every layout for a job — cut file, annotated verbose copy, print layer |
| `slider.json` | `Define`/`Call`, `If`, `Assert` and `SetData` — the slider's two corner variants, and nest spacing chosen from the result |
| `corner_radius_proof.json` | asserts that an inset+outset really is a corner radius — area comes out `12 - (4-pi)r^2` |
| `dynamic_groups.json` | dynamic group names (`kerf-{n}`), computed slot indexes (`set_index: "n"`) and templated output paths (`sheet{n + 1}.svg`) |
| `artwork/rect.svg` | one 6x2in rectangle, area 12 sq in — the reference for `corner_radius_proof.json` |
| `artwork/two_rects.svg` | 6x2in and 4x1in parts. The small one is there **on purpose**: it cannot survive a 0.63in radius, so `slider.json`'s `Assert` fires |

## Notes

* JSON strings cannot contain raw newlines, so the metadata block is one line.
  The UI will generate that naturally; hand-editing is where it bites.
* `--debug` makes a failed `Assert` stop the run. Without it asserts are
  collected and printed as warnings, which is how AWS behaves. Try both on
  `slider.json` — same program, same geometry:

      gel-cli run         programs/slider.json programs/artwork/two_rects.svg
      gel-cli run --debug programs/slider.json programs/artwork/two_rects.svg

* `CornerRadius` is deliberately **not** an instruction. Rounding convex
  corners is an inset then an outset with a round join, which `slider.json`
  shows. Verified exact: a 6x2in rectangle at r=0.63 comes out with area
  `12 - (4 - pi)r^2`.

* Keep these programs runnable. Everything here was rebuilt once after being
  left in a scratch directory that got wiped - a layout program is a
  deliverable, not a temp file.
