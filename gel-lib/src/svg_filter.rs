//! General-purpose "strip elements matching an attribute" SVG filter.
//!
//! Motivated by a real problem found while attempting a layout with real
//! `SIGN TYPE *.cdr` -> SVG files: LibreOffice's CDR->SVG export inserts a
//! synthetic helper rectangle for every real shape, marked
//! `class="BoundingBox"` - purely so the overall canvas size can be
//! recomputed downstream (see asg-quote-cost's cdrConverter.js), never
//! meant to be real drawing content. `depth_tree`'s SVG importer has no
//! awareness of this and imports every `<path>`/`<rect>`/etc unconditionally,
//! so these synthetic rectangles end up interleaved with real content at
//! every depth of the shape tree (a bounding box always geometrically
//! contains its own real shape, so depth_tree's containment-based tree
//! build naturally nests them right alongside it) - which is exactly why
//! filtering by depth alone kept surfacing plain rectangles instead of
//! real artwork.
//!
//! Fixed here, in `gel` itself, rather than in `depth_tree` (shared with
//! `ivy`, so changing its behavior needs more care) - and deliberately
//! general rather than hardcoded to `class="BoundingBox"` specifically:
//! this is a rule list (attribute name -> value to match), so any other
//! synthetic-artifact marker found later (a different class, an id
//! pattern, whatever) is a config change, not a code change. Runs as its
//! own SVG-in -> SVG-out step, meant to sit between "convert CDR to SVG"
//! and "load the SVG into gel's Data" - not folded into either.

use quick_xml::Reader;
use quick_xml::Writer;
use quick_xml::events::{BytesStart, Event};
use std::io::Cursor;

/// Strip any element (and its entire subtree) whose attribute `attr`
/// equals `value` exactly, for at least one rule in `rules`.
#[derive(Debug, Clone)]
pub struct StripRule {
    pub attr: String,
    pub value: String,
}

impl StripRule {
    pub fn new(attr: impl Into<String>, value: impl Into<String>) -> Self {
        Self { attr: attr.into(), value: value.into() }
    }
}

/// The known real-world artifact rules today - LibreOffice's per-shape
/// bounding-box helper rectangles. Extend this list (or pass your own via
/// [`strip_elements`] directly) as new synthetic-artifact markers turn up
/// in other export paths - this isn't meant to be the only rule ever needed.
pub fn known_artifact_rules() -> Vec<StripRule> {
    vec![StripRule::new("class", "BoundingBox")]
}

/// Result of a filter pass: the cleaned SVG plus how many elements were
/// actually stripped, so callers/tools can report "removed N artifacts"
/// instead of silently transforming the file.
#[derive(Debug, Clone)]
pub struct StripResult {
    pub svg: String,
    pub elements_stripped: usize,
}

/// Removes every element matching any rule in `rules`, along with its
/// full subtree (an artifact element is never assumed to be childless).
pub fn strip_elements(svg: &str, rules: &[StripRule]) -> Result<StripResult, String> {
    let mut reader = Reader::from_str(svg);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Cursor::new(Vec::new()));

    // >0 while inside a to-be-stripped element's subtree; tracks nesting
    // depth so a stripped element's own children aren't re-emitted once
    // we've decided to drop it, and so we know exactly when we've left it.
    let mut skip_depth: u32 = 0;
    let mut elements_stripped = 0usize;

    loop {
        let event = reader.read_event().map_err(|e| format!("XML read error: {e}"))?;
        match event {
            Event::Eof => break,
            Event::Start(ref e) => {
                if skip_depth > 0 {
                    skip_depth += 1;
                    continue;
                }
                if matches_any_rule(e, rules) {
                    skip_depth = 1;
                    elements_stripped += 1;
                    continue;
                }
                writer.write_event(Event::Start(e.clone())).map_err(|e| e.to_string())?;
            }
            Event::Empty(ref e) => {
                if skip_depth > 0 {
                    continue;
                }
                if matches_any_rule(e, rules) {
                    elements_stripped += 1;
                    continue;
                }
                writer.write_event(Event::Empty(e.clone())).map_err(|e| e.to_string())?;
            }
            Event::End(ref e) => {
                if skip_depth > 0 {
                    skip_depth -= 1;
                    continue;
                }
                writer.write_event(Event::End(e.clone())).map_err(|e| e.to_string())?;
            }
            other => {
                if skip_depth > 0 {
                    continue;
                }
                writer.write_event(other).map_err(|e| e.to_string())?;
            }
        }
    }

    let bytes = writer.into_inner().into_inner();
    let svg = String::from_utf8(bytes).map_err(|e| format!("filtered SVG was not valid UTF-8: {e}"))?;
    Ok(StripResult { svg, elements_stripped })
}

/// Convenience wrapper over [`strip_elements`] using [`known_artifact_rules`].
pub fn strip_known_artifacts(svg: &str) -> Result<StripResult, String> {
    strip_elements(svg, &known_artifact_rules())
}

fn matches_any_rule(start: &BytesStart, rules: &[StripRule]) -> bool {
    for rule in rules {
        let found = start.attributes().flatten().any(|a| {
            a.key.as_ref() == rule.attr.as_str() && a.value.as_ref() == rule.value.as_str()
        });
        if found {
            return true;
        }
    }
    false
}
