//! `Assert` - check an expectation about the geometry.
//!
//! The only instruction whose failure is not automatically fatal, and that is
//! deliberate. Everywhere else a failure means the *program* is wrong - a
//! typo'd colour, an unknown group, a misspelled condition - and stopping is
//! right. An assert failing means the *geometry* is surprising, and a laser
//! job with one questionable part is still worth cutting: the operator can
//! look at the flagged part, whereas halting hands them nothing.
//!
//! So: debug mode halts (you are developing the program, you want the stack),
//! production collects and carries on (the layout still ships, and the
//! warning is returned for the end user).

use serde::{Deserialize, Serialize};

use crate::*;

/// Fails loudly in debug, collects a [`Warning`] in production.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assert {
    /// Stable identifier, so repeated failures can be grouped and counted.
    pub id: String,
    /// Expression that should be true.
    pub condition: String,
    /// Template; `{expression}` expands. Put the context in here - "failed
    /// x13" says nothing, "part lost on sheet 2" says everything - because
    /// only the program knows what context is worth capturing.
    pub message: String,
}

impl Query for Assert {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let holds = {
            let value = data
                .context
                .eval(boa_engine::Source::from_bytes(&self.condition))
                .map_err(|err| {
                    format!(
                        "Assert '{}': could not evaluate condition ({:?}): {err}",
                        self.id, self.condition
                    )
                })?;
            value.to_boolean()
        };
        if holds {
            return Ok(());
        }

        let message = resolve_template(data, &self.message, "Assert.message")?;
        let group = data.current_group.lock().unwrap().clone();

        if data.debug {
            return Err(format!("Assert '{}' failed: {message}", self.id));
        }

        let mut warnings = data.warnings.lock().unwrap();
        let sequence = warnings.len();
        warnings.push(Warning {
            sequence,
            id: self.id.clone(),
            message,
            group,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::*;
    use geo::{LineString, Polygon};

    fn data() -> Data {
        let square = Polygon::new(
            LineString::from(vec![(0.0_f64, 0.0_f64), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)]),
            vec![],
        );
        let mut data = Data::from(vec![square]);
        data.sync_style_lengths();
        RunCode { code: "n = 2;".into() }.query(&mut data).unwrap();
        data
    }

    fn assert_instruction() -> Assert {
        Assert {
            id: "corner_radius_survived".into(),
            condition: "false".into(),
            message: "part lost on sheet {n}".into(),
        }
    }

    #[test]
    fn a_holding_assert_does_nothing() {
        let mut data = data();
        Assert {
            condition: "n == 2".into(),
            ..assert_instruction()
        }
        .query(&mut data)
        .expect("should pass");
        assert!(data.warnings.lock().unwrap().is_empty());
    }

    /// Production: the layout still ships, and the warning comes back with
    /// the program's own context expanded into it.
    #[test]
    fn production_collects_and_continues() {
        let mut data = data();
        assert!(!data.debug, "production must be the default");

        assert_instruction().query(&mut data).expect("must not halt in production");

        let warnings = data.warnings.lock().unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].id, "corner_radius_survived");
        assert_eq!(warnings[0].message, "part lost on sheet 2");
        assert_eq!(warnings[0].sequence, 0);
    }

    #[test]
    fn debug_halts() {
        let mut data = data();
        data.debug = true;

        let err = assert_instruction()
            .query(&mut data)
            .expect_err("debug mode should stop the run");
        assert!(err.contains("corner_radius_survived"), "got: {err}");
        assert!(err.contains("part lost on sheet 2"), "got: {err}");
        assert!(data.warnings.lock().unwrap().is_empty(), "halting should not also collect");
    }

    /// Repeats are kept individually so the caller can count them; the `id`
    /// is what groups them.
    #[test]
    fn repeats_are_numbered_not_merged() {
        let mut data = data();
        for _ in 0..3 {
            assert_instruction().query(&mut data).unwrap();
        }
        let warnings = data.warnings.lock().unwrap();
        assert_eq!(warnings.len(), 3);
        assert_eq!(
            warnings.iter().map(|w| w.sequence).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    /// `shape_count` exists because this guard is the natural one to write,
    /// and the slot-based version of it silently passed: `SetOp append` adds
    /// an empty slot for a part that vanished, so slot counts matched while a
    /// shape was missing.
    #[test]
    fn shape_count_totals_a_group_across_slots() {
        let mut data = data();
        {
            let mut groups = data.groups.lock().unwrap();
            groups.insert("src".into(), vec![vec![0], vec![0]]);
            // What a lost part looks like: same number of slots, one empty.
            groups.insert("out".into(), vec![vec![0], vec![]]);
        }

        let mut check = |condition: &str| {
            Assert {
                id: "survived".into(),
                condition: condition.into(),
                message: "lost a part".into(),
            }
            .query(&mut data)
            .unwrap();
            data.warnings.lock().unwrap().len()
        };

        // The slot-based guard passes - this is the bug.
        assert_eq!(check("len('out') == len('src')"), 0);
        // The shape-based one catches it.
        assert_eq!(check("shape_count('out') == shape_count('src')"), 1);
    }

    #[test]
    fn shape_count_rejects_an_unknown_group() {
        let mut data = data();
        let err = Assert {
            id: "x".into(),
            condition: "shape_count('nope') > 0".into(),
            message: "".into(),
        }
        .query(&mut data)
        .expect_err("an unknown group must not count as zero and pass silently");
        assert!(err.contains("nope"), "got: {err}");
    }
}
