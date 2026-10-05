use boa_engine::Source;
use serde::{Deserialize, Serialize};

use crate::*;

/// Runs arbitrary JavaScript against the shared context.
///
/// Every other instruction reads and writes *groups*; this one reads and
/// writes the JS context instead, which is why it has no `get_group`/
/// `set_group`. It's how a program defines constants, helper predicates and
/// loop counters:
///
/// ```json
/// {"RunCode": {"code": "thou = 0.001;\noffset = 4 * thou;"}}
/// ```
///
/// Definitions persist for the rest of the program, because `Data` holds one
/// Boa context reused by every instruction - the same mechanism `Filter`
/// already relies on for its predicates.
///
/// Errors are NOT swallowed. A typo that left `offset` undefined would make
/// every later `offset(...)` silently produce `undefined`, and parts would be
/// cut at the wrong size with nothing reported.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunCode {
    pub code: String,
}

impl Query for RunCode {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        data.context
            .eval(Source::from_bytes(&self.code))
            .map(|_| ())
            .map_err(|err| format!("RunCode failed: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use geo::polygon;

    use crate::*;

    #[test]
    fn defines_values_visible_to_later_instructions() {
        let mut data = Data::from(vec![polygon! {
            (x: 0.0, y: 0.0), (x: 1.0, y: 0.0), (x: 1.0, y: 1.0), (x: 0.0, y: 1.0)
        }]);

        let instructions = vec![
            Instruction::RunCode(RunCode {
                code: "min_area = 0.5;".into(),
            }),
            // The filter can only pass if `min_area` survived from the RunCode.
            Instruction::Filter(Filter {
                set_group: "big".into(),
                get_group: "main".into(),
                code: "area(i) > min_area".into(),
            }),
        ];

        data.query(instructions).expect("program should run");

        let groups = data.groups.lock().unwrap();
        assert_eq!(groups.get("big"), Some(&vec![vec![0]]));
    }

    #[test]
    fn a_javascript_error_fails_the_program() {
        let mut data = Data::from(vec![polygon! {(x: 0.0, y: 0.0)}]);
        let mut run = RunCode {
            code: "this is not valid javascript".into(),
        };
        assert!(run.query(&mut data).is_err());
    }
}
