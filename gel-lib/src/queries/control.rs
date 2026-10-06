//! `If` and `While` - conditional and repeated execution.
//!
//! The condition is a JS expression evaluated against the shared context, and
//! its **truthiness** decides - the same rule JS's own `if` uses, since the
//! expression is JS. A name that does not exist raises a `ReferenceError`
//! rather than reading as false, so a typo'd condition stops the program
//! instead of silently skipping the body.

use boa_engine::Source;
use serde::{Deserialize, Serialize};

use crate::*;

/// Evaluates a condition, propagating errors rather than defaulting to false.
fn truthy(data: &mut Data, expression: &str, field: &str) -> Result<bool, String> {
    let value = data
        .context
        .eval(Source::from_bytes(expression))
        .map_err(|err| format!("could not evaluate {field} ({expression:?}): {err}"))?;
    Ok(value.to_boolean())
}

/// Runs its instructions when the condition holds.
///
/// This is what lets a rule apply to some signs and not others - the laser
/// guide is full of them ("if not thermoformed, offset the slider face by
/// 0.005"", "make one version with radial corners"). The metadata those
/// conditions read is prepended to the program as a `RunCode`, so there is no
/// `meta_data()` accessor to learn.
///
/// **No `else`.** Two `If`s with opposite conditions say the same thing with
/// one less concept in the ISA, and they read the same in JSON:
///
/// ```json
/// {"If": {"condition": "radial_corners",  "instructions": [ ... ]}}
/// {"If": {"condition": "!radial_corners", "instructions": [ ... ]}}
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct If {
    pub condition: String,
    pub instructions: Vec<Instruction>,
}

impl Query for If {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        if !truthy(data, &self.condition, "If.condition")? {
            return Ok(());
        }
        for instruction in self.instructions.iter_mut() {
            instruction
                .query(data)
                .map_err(|err| format!("in If({:?}) -> {err}", self.condition))?;
        }
        Ok(())
    }
}

/// How many times a `While` may go round before giving up.
const DEFAULT_MAX_ITERATIONS: usize = 1000;

fn default_max_iterations() -> usize {
    DEFAULT_MAX_ITERATIONS
}

/// Repeats while a condition holds.
///
/// `LoopOver` walks a group's slots, so it cannot count. `While` can, which
/// is what an "offset N times into N groups" series needs - with dynamic
/// group names that is a loop body rather than its own instruction, so
/// `OffsetSeries` never had to be built.
///
/// Always bounded. A condition that never goes false is a bug, and a layout
/// job that hangs on AWS is worse than one that fails.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct While {
    pub condition: String,
    pub instructions: Vec<Instruction>,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
}

impl Query for While {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let mut count = 0usize;
        while truthy(data, &self.condition, "While.condition")? {
            if count >= self.max_iterations {
                return Err(format!(
                    "While({:?}) ran {} times without the condition going false",
                    self.condition, self.max_iterations
                ));
            }
            count += 1;
            for instruction in self.instructions.iter_mut() {
                instruction
                    .query(data)
                    .map_err(|err| format!("in While (pass {count}) -> {err}"))?;
            }
        }
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
        let data = Data::from(vec![square]);
        data.sync_style_lengths();
        data
    }

    fn set(data: &mut Data, code: &str) {
        RunCode { code: code.into() }.query(data).unwrap();
    }

    fn read(data: &mut Data, expression: &str) -> String {
        data.context
            .eval(boa_engine::Source::from_bytes(expression))
            .unwrap()
            .to_string(&mut data.context)
            .unwrap()
            .to_std_string_lossy()
    }

    /// Two `If`s with opposite conditions are how a program branches, now
    /// that there is no `else`. Exactly one must fire.
    #[test]
    fn opposite_conditions_pick_exactly_one_branch() {
        let mut data = data();
        set(&mut data, "thermoformed = false; took = 'neither';");

        for (condition, marker) in [("thermoformed", "then"), ("!thermoformed", "else")] {
            If {
                condition: condition.into(),
                instructions: vec![Instruction::RunCode(RunCode {
                    code: format!("took = '{marker}';"),
                })],
            }
            .query(&mut data)
            .expect("if should run");
        }

        assert_eq!(read(&mut data, "took"), "else");
    }

    /// The guide's rule: offset the slider face by 0.005" only when it was
    /// not thermoformed.
    #[test]
    fn if_without_an_else_is_a_no_op_when_false() {
        let mut data = data();
        set(&mut data, "thermoformed = true;");

        If {
            condition: "thermoformed != true".into(),
            instructions: vec![Instruction::Offset(Offset {
                get_group: "main".into(),
                get_index: Index::Literal(0),
                set_group: "face".into(),
                set_index: Index::Literal(0),
                amount: "0.005".into(),
                join: None,
                arc_tolerance: Some("0.0001".into()),
                miter_limit: None,
                join_value: None,
            })],
        }
        .query(&mut data)
        .expect("if should run");

        let groups = data.groups.lock().unwrap();
        assert!(!groups.contains_key("face"), "the branch must not have run");
    }

    /// A typo'd condition must stop the program, not read as false and
    /// silently skip the body.
    #[test]
    fn an_unknown_name_in_a_condition_is_an_error() {
        let mut data = data();
        let err = If {
            condition: "thermofrmed".into(),
            instructions: vec![],
        }
        .query(&mut data)
        .expect_err("a misspelled condition must not quietly be false");
        assert!(err.contains("If.condition"), "got: {err}");
    }

    /// The counted loop `LoopOver` cannot express - this is why
    /// `OffsetSeries` was never needed.
    #[test]
    fn while_counts_and_writes_a_group_per_pass() {
        let mut data = data();
        set(&mut data, "n = 0; arc_tolerance = 0.1 * 0.001;");

        While {
            condition: "n < 3".into(),
            max_iterations: 1000,
            instructions: vec![
                Instruction::Offset(Offset {
                    get_group: "main".into(),
                    get_index: Index::Literal(0),
                    set_group: "ring-{n}".into(),
                    set_index: Index::Literal(0),
                    amount: "0.063 * (n + 1)".into(),
                    join: None,
                    arc_tolerance: None,
                    miter_limit: None,
                    join_value: None,
                }),
                Instruction::RunCode(RunCode { code: "n = n + 1;".into() }),
            ],
        }
        .query(&mut data)
        .expect("while should run");

        let groups = data.groups.lock().unwrap();
        for pass in 0..3 {
            assert!(
                groups.contains_key(&format!("ring-{pass}")),
                "pass {pass} should have written its own group; got {:?}",
                groups.keys()
            );
        }
        assert!(!groups.contains_key("ring-3"), "should have stopped at 3");
    }

    #[test]
    fn a_runaway_while_is_bounded() {
        let mut data = data();
        set(&mut data, "n = 0;");
        let err = While {
            condition: "true".into(),
            max_iterations: 10,
            instructions: vec![Instruction::RunCode(RunCode { code: "n = n + 1;".into() })],
        }
        .query(&mut data)
        .expect_err("an endless loop must fail, not hang a job on AWS");
        assert!(err.contains("without the condition going false"), "got: {err}");
    }
}
