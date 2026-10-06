//! Expression evaluation shared by every instruction.
//!
//! The ISA rule is that a field is a *program*, not a constant: numbers are
//! JS expressions, and group names are templates. That is what lets a
//! `LoopOver` body do different work on each pass rather than overwriting
//! itself.

use boa_engine::{JsValue, Source};
use serde::{Deserialize, Serialize};

use crate::*;

/// Evaluates an expression against the shared JS context, as a number.
pub fn eval_number(data: &mut Data, expression: &str, field: &str) -> Result<f64, String> {
    let value = data
        .context
        .eval(Source::from_bytes(expression))
        .map_err(|err| format!("could not evaluate {field} ({expression:?}): {err}"))?;
    match value {
        JsValue::Integer(n) => Ok(n as f64),
        JsValue::Rational(n) => Ok(n),
        other => Err(format!(
            "{field} ({expression:?}) evaluated to {other:?}, which is not a number"
        )),
    }
}

/// Reads a named setting from the shared JS context, if the program set one.
///
/// A program declares its tunables once, up front, in a `RunCode`, and every
/// instruction that wants one resolves it by name:
///
/// ```json
/// {"RunCode": {"code": "thou = 0.001; arc_tolerance = 0.1 * thou;"}}
/// ```
///
/// So changing the job means editing one line rather than hunting a magic
/// number through every instruction.
///
/// `Ok(None)` means "no such setting". That has to be distinguishable from
/// an expression that evaluated to something unusable, because the caller
/// needs to report "this field has no value and cannot be defaulted" with an
/// actionable message rather than a `ReferenceError`.
pub fn setting_number(data: &mut Data, name: &str) -> Result<Option<f64>, String> {
    // `typeof x` is the one way to ask about an identifier that may not
    // exist: it yields `"undefined"` instead of throwing a ReferenceError.
    let kind = data
        .context
        .eval(Source::from_bytes(&format!("typeof {name}")))
        .and_then(|kind| kind.to_string(&mut data.context))
        .map_err(|err| format!("could not read setting {name}: {err}"))?
        .to_std_string_lossy();
    if kind == "undefined" {
        return Ok(None);
    }
    eval_number(data, name, &format!("setting {name}")).map(Some)
}

/// Evaluates an expression against the shared JS context, as a string.
pub fn eval_string(data: &mut Data, expression: &str, field: &str) -> Result<String, String> {
    let value = data
        .context
        .eval(Source::from_bytes(expression))
        .map_err(|err| format!("could not evaluate {field} ({expression:?}): {err}"))?;
    value
        .to_string(&mut data.context)
        .map(|s| s.to_std_string_lossy())
        .map_err(|err| format!("{field} ({expression:?}) is not printable: {err}"))
}

/// A slot number: either written literally (`2`) or computed (`"n"`).
///
/// Untagged, so both JSON spellings parse and every program written before
/// this existed keeps working.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Index {
    Literal(usize),
    Expression(String),
}

impl Default for Index {
    fn default() -> Self {
        Index::Literal(0)
    }
}

impl From<usize> for Index {
    fn from(n: usize) -> Self {
        Index::Literal(n)
    }
}

impl Index {
    /// A negative or fractional result is an error rather than a silent
    /// truncation to slot 0 - writing every pass of a loop into the same
    /// slot would look like the instruction simply did nothing.
    pub fn resolve(&self, data: &mut Data, field: &str) -> Result<usize, String> {
        match self {
            Index::Literal(n) => Ok(*n),
            Index::Expression(expression) => {
                let value = eval_number(data, expression, field)?;
                if value < 0.0 || value.fract() != 0.0 {
                    return Err(format!(
                        "{field} ({expression:?}) evaluated to {value}, which is not a slot number"
                    ));
                }
                Ok(value as usize)
            }
        }
    }
}

/// Resolves a group name, expanding `{expression}` against the JS context.
///
/// `"panels"` is itself a valid name, so a bare string cannot be evaluated as
/// code - it would have to be quoted, breaking every existing program.
/// Braces mark the computed part instead: `"sheet-{n}"` -> `"sheet-0"`.
/// Write `{{` for a literal brace.
pub fn resolve_template(data: &mut Data, template: &str, field: &str) -> Result<String, String> {
    if !template.contains('{') && !template.contains('}') {
        return Ok(template.to_string());
    }

    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut expression = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    expression.push(c);
                }
                if !closed {
                    return Err(format!(
                        "{field} ({template:?}) has a '{{' with no matching '}}'"
                    ));
                }
                out.push_str(&eval_string(data, &expression, field)?);
            }
            // A lone '}' is a typo, not a literal - catching it here stops a
            // mistyped template from silently naming the wrong group.
            '}' => {
                return Err(format!(
                    "{field} ({template:?}) has a '}}' with no matching '{{' (write '}}}}' for a literal)"
                ))
            }
            _ => out.push(c),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use crate::*;

    fn data() -> Data {
        use geo::{LineString, Polygon};
        let shapes: Vec<Polygon<f64>> = vec![Polygon::new(
            LineString::from(vec![(0.0_f64, 0.0_f64), (1.0, 0.0), (1.0, 1.0)]),
            vec![],
        )];
        let mut data = Data::from(shapes);
        RunCode {
            code: "n = 3; name = 'panels';".into(),
        }
        .query(&mut data)
        .unwrap();
        data
    }

    #[test]
    fn a_plain_name_is_not_evaluated() {
        let mut data = data();
        assert_eq!(resolve_template(&mut data, "cut_profile", "f").unwrap(), "cut_profile");
    }

    #[test]
    fn braces_expand_against_the_context() {
        let mut data = data();
        assert_eq!(resolve_template(&mut data, "sheet-{n}", "f").unwrap(), "sheet-3");
        assert_eq!(resolve_template(&mut data, "{name}", "f").unwrap(), "panels");
        assert_eq!(resolve_template(&mut data, "a{n}b{n + 1}c", "f").unwrap(), "a3b4c");
        assert_eq!(resolve_template(&mut data, "{{literal}}", "f").unwrap(), "{literal}");
    }

    #[test]
    fn an_unclosed_brace_is_an_error() {
        let mut data = data();
        assert!(resolve_template(&mut data, "sheet-{n", "f").is_err());
    }

    #[test]
    fn an_index_is_a_literal_or_an_expression() {
        let mut data = data();
        assert_eq!(Index::Literal(2).resolve(&mut data, "f").unwrap(), 2);
        assert_eq!(
            Index::Expression("n + 1".into()).resolve(&mut data, "f").unwrap(),
            4
        );
        // Both JSON spellings parse.
        assert_eq!(
            serde_json::from_str::<Index>("2").unwrap().resolve(&mut data, "f").unwrap(),
            2
        );
        assert_eq!(
            serde_json::from_str::<Index>("\"n\"").unwrap().resolve(&mut data, "f").unwrap(),
            3
        );
    }

    /// The whole point of both features: a loop body that writes somewhere
    /// different on each pass instead of overwriting itself.
    #[test]
    fn a_loop_writes_a_distinct_group_and_slot_each_pass() {
        use geo::{LineString, Polygon};
        let square = |x: f64| {
            Polygon::new(
                LineString::from(vec![
                    (x, 0.0),
                    (x + 1.0, 0.0),
                    (x + 1.0, 1.0),
                    (x, 1.0),
                    (x, 0.0),
                ]),
                vec![],
            )
        };
        let mut data = Data::from(vec![square(0.0), square(10.0), square(20.0)]);
        {
            let mut groups = data.groups.lock().unwrap();
            groups.insert("parts".into(), vec![vec![0], vec![1], vec![2]]);
        }
        RunCode { code: "n = 0;".into() }.query(&mut data).unwrap();

        LoopOver {
            get_group: "parts".into(),
            iterator_name: "part".into(),
            instructions: vec![
                Instruction::Copy(Copy {
                    get_group: "part".into(),
                    get_index: Index::Literal(0),
                    // Both at once: a computed group name and a computed slot.
                    set_group: "stacked-{n}".into(),
                    set_index: Index::Expression("n".into()),
                }),
                Instruction::RunCode(RunCode {
                    code: "n = n + 1;".into(),
                }),
            ],
        }
        .query(&mut data)
        .expect("loop should run");

        let groups = data.groups.lock().unwrap();
        for pass in 0..3 {
            let group = groups
                .get(&format!("stacked-{pass}"))
                .unwrap_or_else(|| panic!("pass {pass} should have written its own group"));
            // Slot `pass`, so earlier slots are empty padding - the write
            // landed where the expression said, not at slot 0.
            assert_eq!(group.len(), pass + 1, "pass {pass}");
            assert_eq!(group[pass].len(), 1, "pass {pass}");
        }
    }

    #[test]
    fn a_negative_slot_is_an_error_not_slot_zero() {
        let mut data = data();
        let err = Index::Expression("n - 10".into())
            .resolve(&mut data, "f")
            .expect_err("a negative slot must not silently become 0");
        assert!(err.contains("not a slot number"), "got: {err}");
    }
}
