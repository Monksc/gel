//! `Define` and `Call` - named, parameterised instruction sequences.
//!
//! Boa already gives a program reusable *expressions* (`RunCode` can define a
//! JS function). This is the other half: reusable *instruction sequences*, so
//! the same six-step offset-and-tag recipe isn't copied into four layout
//! files.
//!
//! There is no `Include`. The quote generator prepends the shared library of
//! `Define`s onto each layout program before upload, so composition is array
//! concatenation and the server runs one self-contained file.
//!
//! Parameters are bound as JS variables, so the body reads them through the
//! same `{expression}` templates and expression fields every other
//! instruction already uses - no second substitution engine.

use boa_engine::{JsValue, Source, js_string, property::Attribute};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::*;

/// Registers a named instruction sequence. Runs nothing itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Define {
    pub name: String,
    #[serde(default)]
    pub params: Vec<String>,
    pub instructions: Vec<Instruction>,
}

impl Query for Define {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let mut functions = data.functions.lock().unwrap();
        // Redefining silently would make a layout file's own helper quietly
        // shadow a library one with the same name - the composed program is
        // machine-generated, so a collision is a bug in composition, not an
        // intentional override.
        if functions.contains_key(&self.name) {
            return Err(format!("Define: '{}' is already defined", self.name));
        }
        functions.insert(
            self.name.clone(),
            FunctionDef {
                params: self.params.clone(),
                instructions: self.instructions.clone(),
            },
        );
        Ok(())
    }
}

/// How deep `Call` may nest before giving up.
const MAX_CALL_DEPTH: usize = 64;

/// Runs a `Define`d sequence with arguments bound as JS variables.
///
/// There is no return value, because groups are the registers: a function
/// writes to whichever group the caller names, the same way a routine writes
/// to the register its caller nominated.
///
/// Argument typing follows one rule. A string that is *entirely* one
/// `{expression}` binds that expression's value with its type intact
/// (`"{base * 2}"` is the number 0.008); any other string is a template
/// producing a string (`"sheet-{n}"` is `"sheet-0"`); non-strings bind as-is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Call {
    pub name: String,
    #[serde(default)]
    pub args: Map<String, Value>,
}

impl Query for Call {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let function = {
            let functions = data.functions.lock().unwrap();
            functions
                .get(&self.name)
                .cloned()
                .ok_or_else(|| format!("Call: no function named '{}'", self.name))?
        };

        for param in &function.params {
            if !self.args.contains_key(param) {
                return Err(format!(
                    "Call '{}': missing argument '{param}' (needs {:?})",
                    self.name, function.params
                ));
            }
        }
        for name in self.args.keys() {
            if !function.params.contains(name) {
                return Err(format!(
                    "Call '{}': unexpected argument '{name}' (takes {:?})",
                    self.name, function.params
                ));
            }
        }

        // Evaluate in the CALLER's scope, before anything is rebound.
        let mut bound: Vec<(String, JsValue)> = Vec::new();
        for (name, value) in &self.args {
            let field = format!("Call '{}' arg '{name}'", self.name);
            let value = match value {
                Value::String(text) => match whole_expression(text) {
                    Some(expression) => data
                        .context
                        .eval(Source::from_bytes(expression))
                        .map_err(|err| format!("{field} ({expression:?}): {err}"))?,
                    None => JsValue::from(js_string!(resolve_template(data, text, &field)?)),
                },
                other => JsValue::from_json(other, &mut data.context)
                    .map_err(|err| format!("{field}: {err}"))?,
            };
            bound.push((name.clone(), value));
        }

        let depth = {
            let mut depth = data.call_depth.lock().unwrap();
            *depth += 1;
            if *depth > MAX_CALL_DEPTH {
                *depth -= 1;
                return Err(format!(
                    "Call '{}': nested more than {MAX_CALL_DEPTH} deep - infinite recursion?",
                    self.name
                ));
            }
            *depth
        };

        // Save whatever the caller had under these names. JS globals are flat,
        // so without this a recursive or nested call clobbers its caller's
        // arguments on the way back out.
        let previous: Vec<(String, JsValue)> = bound
            .iter()
            .map(|(name, _)| {
                let existing = data
                    .context
                    .global_object()
                    .get(js_string!(name.as_str()), &mut data.context)
                    .unwrap_or(JsValue::undefined());
                (name.clone(), existing)
            })
            .collect();

        for (name, value) in &bound {
            data.context
                .register_global_property(js_string!(name.as_str()), value.clone(), Attribute::all())
                .map_err(|err| format!("Call '{}': could not bind '{name}': {err}", self.name))?;
        }
        // Lets a body name scratch groups "tmp-{__call}" and not collide with
        // another call site - local variables without a scope system.
        let _ = data.context.register_global_property(
            js_string!("__call"),
            depth as i32,
            Attribute::all(),
        );

        let mut instructions = function.instructions.clone();
        let mut result = Ok(());
        for instruction in instructions.iter_mut() {
            if let Err(err) = instruction.query(data) {
                result = Err(format!("in '{}' -> {err}", self.name));
                break;
            }
        }

        for (name, value) in previous {
            let _ = data.context.register_global_property(
                js_string!(name.as_str()),
                value,
                Attribute::all(),
            );
        }
        *data.call_depth.lock().unwrap() -= 1;

        result
    }
}

/// `Some(inner)` when the whole string is one `{...}`, so its value can be
/// bound with its type intact rather than stringified.
fn whole_expression(text: &str) -> Option<&str> {
    let inner = text.strip_prefix('{')?.strip_suffix('}')?;
    // A nested or second brace means it's a template, not one expression.
    if inner.contains('{') || inner.contains('}') {
        return None;
    }
    Some(inner)
}

#[cfg(test)]
mod tests {
    use crate::*;
    use geo::{LineString, Polygon};
    use serde_json::{json, Map};

    fn data() -> Data {
        let square = |x: f64| {
            Polygon::new(
                LineString::from(vec![(x, 0.0), (x + 2.0, 0.0), (x + 2.0, 2.0), (x, 2.0)]),
                vec![],
            )
        };
        let data = Data::from(vec![square(0.0), square(10.0)]);
        data.sync_style_lengths();
        data
    }

    fn define(data: &mut Data, name: &str, params: &[&str], instructions: Vec<Instruction>) {
        Define {
            name: name.into(),
            params: params.iter().map(|s| s.to_string()).collect(),
            instructions,
        }
        .query(data)
        .expect("Define should succeed");
    }

    /// The point of the whole feature: one body, two call sites, different
    /// groups out.
    #[test]
    fn a_call_writes_to_the_group_the_caller_named() {
        let mut data = data();
        define(
            &mut data,
            "grow",
            &["src", "dst", "amount"],
            vec![Instruction::Offset(Offset {
                get_group: "{src}".into(),
                get_index: Index::Literal(0),
                set_group: "{dst}".into(),
                set_index: Index::Literal(0),
                amount: "amount".into(),
                join: None,
                arc_tolerance: Some("0.0001".into()),
                miter_limit: None,
                join_value: None,
            })],
        );

        Call {
            name: "grow".into(),
            args: json!({"src": "main", "dst": "big", "amount": 1.0})
                .as_object()
                .unwrap()
                .clone(),
        }
        .query(&mut data)
        .expect("call should run");

        let groups = data.groups.lock().unwrap();
        assert!(groups.contains_key("big"), "groups: {:?}", groups.keys());
    }

    /// A number must arrive as a number, not the string "0.008".
    #[test]
    fn a_whole_expression_argument_keeps_its_type() {
        let mut data = data();
        RunCode {
            code: "base = 0.004;".into(),
        }
        .query(&mut data)
        .unwrap();

        define(
            &mut data,
            "check",
            &["kerf", "label"],
            vec![Instruction::RunCode(RunCode {
                code: "saw_number = (typeof kerf === 'number'); saw_label = label;".into(),
            })],
        );

        Call {
            name: "check".into(),
            args: json!({"kerf": "{base * 2}", "label": "sheet-{1 + 1}"})
                .as_object()
                .unwrap()
                .clone(),
        }
        .query(&mut data)
        .expect("call should run");

        let number = data.context.eval(boa_engine::Source::from_bytes("saw_number")).unwrap();
        assert_eq!(number.as_boolean(), Some(true), "kerf should be a number");
        let label = data.context.eval(boa_engine::Source::from_bytes("saw_label")).unwrap();
        assert_eq!(label.to_string(&mut data.context).unwrap().to_std_string_lossy(), "sheet-2");
    }

    #[test]
    fn arguments_are_restored_after_the_call() {
        let mut data = data();
        RunCode {
            code: "src = 'caller-value';".into(),
        }
        .query(&mut data)
        .unwrap();

        define(
            &mut data,
            "noop",
            &["src"],
            vec![Instruction::RunCode(RunCode {
                code: "inner = src;".into(),
            })],
        );
        Call {
            name: "noop".into(),
            args: json!({"src": "callee-value"}).as_object().unwrap().clone(),
        }
        .query(&mut data)
        .expect("call should run");

        let after = data.context.eval(boa_engine::Source::from_bytes("src")).unwrap();
        assert_eq!(
            after.to_string(&mut data.context).unwrap().to_std_string_lossy(),
            "caller-value",
            "the call must not leak its parameter back to the caller"
        );
    }

    #[test]
    fn wrong_arguments_are_caught() {
        let mut data = data();
        define(&mut data, "f", &["a"], vec![]);

        let missing = Call { name: "f".into(), args: Map::new() }
            .query(&mut data)
            .expect_err("missing argument");
        assert!(missing.contains("missing argument 'a'"), "got: {missing}");

        let extra = Call {
            name: "f".into(),
            args: json!({"a": 1, "b": 2}).as_object().unwrap().clone(),
        }
        .query(&mut data)
        .expect_err("unexpected argument");
        assert!(extra.contains("unexpected argument 'b'"), "got: {extra}");

        let unknown = Call { name: "nope".into(), args: Map::new() }
            .query(&mut data)
            .expect_err("unknown function");
        assert!(unknown.contains("no function named 'nope'"), "got: {unknown}");
    }

    #[test]
    fn redefining_a_name_is_an_error() {
        let mut data = data();
        define(&mut data, "f", &[], vec![]);
        let err = Define { name: "f".into(), params: vec![], instructions: vec![] }
            .query(&mut data)
            .expect_err("a composed program must not silently shadow a library function");
        assert!(err.contains("already defined"), "got: {err}");
    }

    #[test]
    fn runaway_recursion_stops() {
        let mut data = data();
        define(
            &mut data,
            "loop_forever",
            &[],
            vec![Instruction::Call(Call { name: "loop_forever".into(), args: Map::new() })],
        );
        let err = Call { name: "loop_forever".into(), args: Map::new() }
            .query(&mut data)
            .expect_err("should hit the depth cap");
        assert!(err.contains("infinite recursion"), "got: {err}");
    }
}
