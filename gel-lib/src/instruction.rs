use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::*;

/// One instruction in a layout program.
///
/// Serialised externally tagged - `{"Filter": {...}}` - which is the shape
/// ivy's saved settings already use, so existing files keep working.
///
/// A program is a `Vec<Instruction>`, run with [`Data::query`]. Instructions
/// read and write named *groups*; the groups are the registers. The one
/// exception is [`RunCode`], which acts on the shared JS context instead.
///
/// `LoopOver` nests a `Vec<Instruction>`, so control flow composes: this is
/// what lets a program do per-sign work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Instruction {
    Filter(Filter),
    Sort(Sort),
    GroupBy(GroupBy),
    Transformation(Transformation),
    Kerning(Kerning),
    LoopOver(LoopOver<Instruction>),
    If(If),
    While(While),
    Assert(Assert),
    RunCode(RunCode),
    Offset(Offset),
    Union(Union),
    Intersect(Intersect),
    Difference(Difference),
    Flatten(Flatten),
    Copy(Copy),
    SetOp(SetOp),
    SetData(SetData),
    Define(Define),
    Call(Call),
    AddShape(AddShape),
    AddText(AddText),
    Emit(Emit),
    /// Only present with the `nest` feature. Without it a program using
    /// `Nest` fails to parse ("unknown variant"), rather than running and
    /// quietly producing an unnested layout.
    #[cfg(feature = "nest")]
    Nest(Nest),
}

/// Per-instruction counts and elapsed time, collected when `GEL_PROFILE` is
/// set in the environment.
///
/// Exists because "this program is slow" is not actionable: the mill/print
/// classification took 63s against 3.5s for nest-and-emit, and the question
/// of whether that is more instructions or slower ones is not answerable by
/// reading the program.
static PROFILE: Mutex<Option<HashMap<&'static str, (usize, Duration)>>> = Mutex::new(None);

fn profiling() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("GEL_PROFILE").is_ok())
}

/// Counts and total time per instruction, slowest first. Empty unless
/// `GEL_PROFILE` was set.
pub fn profile_report() -> Vec<(&'static str, usize, Duration)> {
    let profile = PROFILE.lock().unwrap();
    let Some(counts) = profile.as_ref() else {
        return Vec::new();
    };
    let mut rows: Vec<(&'static str, usize, Duration)> =
        counts.iter().map(|(name, (n, d))| (*name, *n, *d)).collect();
    rows.sort_by(|a, b| b.2.cmp(&a.2));
    rows
}

impl Query for Instruction {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        if !profiling() {
            let result = self.run(data);
            data.sync_style_lengths();
            return result;
        }

        let name = self.name();
        let started = Instant::now();
        let result = self.run(data);
        // Nested instructions are timed inside their own call too, so a
        // LoopOver's total includes its body - read it as inclusive.
        let elapsed = started.elapsed();
        {
            let mut profile = PROFILE.lock().unwrap();
            let counts = profile.get_or_insert_with(HashMap::new);
            let entry = counts.entry(name).or_insert((0, Duration::ZERO));
            entry.0 += 1;
            entry.1 += elapsed;
        }
        // Any instruction may have appended derived shapes; keep the style
        // vectors in step so `Emit` can address them. See
        // [`Data::sync_style_lengths`].
        data.sync_style_lengths();
        result
    }
}

impl Instruction {
    fn run(&mut self, data: &mut Data) -> Result<(), String> {
        match self {
            Instruction::Filter(q) => q.query(data),
            Instruction::Sort(q) => q.query(data),
            Instruction::GroupBy(q) => q.query(data),
            Instruction::Transformation(q) => q.query(data),
            Instruction::Kerning(q) => q.query(data),
            Instruction::LoopOver(q) => q.query(data),
            Instruction::If(q) => q.query(data),
            Instruction::While(q) => q.query(data),
            Instruction::Assert(q) => q.query(data),
            Instruction::RunCode(q) => q.query(data),
            Instruction::Offset(q) => q.query(data),
            Instruction::Union(q) => q.query(data),
            Instruction::Intersect(q) => q.query(data),
            Instruction::Difference(q) => q.query(data),
            Instruction::Flatten(q) => q.query(data),
            Instruction::Copy(q) => q.query(data),
            Instruction::SetOp(q) => q.query(data),
            Instruction::SetData(q) => q.query(data),
            Instruction::Define(q) => q.query(data),
            Instruction::Call(q) => q.query(data),
            Instruction::AddShape(q) => q.query(data),
            Instruction::AddText(q) => q.query(data),
            Instruction::Emit(q) => q.query(data),
            #[cfg(feature = "nest")]
            Instruction::Nest(q) => q.query(data),
        }
    }
}

impl Instruction {
    /// The variant's name, for error messages and run logs.
    pub fn name(&self) -> &'static str {
        match self {
            Instruction::Filter(_) => "Filter",
            Instruction::Sort(_) => "Sort",
            Instruction::GroupBy(_) => "GroupBy",
            Instruction::Transformation(_) => "Transformation",
            Instruction::Kerning(_) => "Kerning",
            Instruction::LoopOver(_) => "LoopOver",
            Instruction::If(_) => "If",
            Instruction::While(_) => "While",
            Instruction::Assert(_) => "Assert",
            Instruction::RunCode(_) => "RunCode",
            Instruction::Offset(_) => "Offset",
            Instruction::Union(_) => "Union",
            Instruction::Intersect(_) => "Intersect",
            Instruction::Difference(_) => "Difference",
            Instruction::Flatten(_) => "Flatten",
            Instruction::Copy(_) => "Copy",
            Instruction::SetOp(_) => "SetOp",
            Instruction::SetData(_) => "SetData",
            Instruction::Define(_) => "Define",
            Instruction::Call(_) => "Call",
            Instruction::AddShape(_) => "AddShape",
            Instruction::AddText(_) => "AddText",
            Instruction::Emit(_) => "Emit",
            #[cfg(feature = "nest")]
            Instruction::Nest(_) => "Nest",
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::*;

    #[test]
    fn round_trips_through_json() {
        let program = vec![
            Instruction::RunCode(RunCode {
                code: "min_area = 40;".into(),
            }),
            Instruction::Filter(Filter {
                set_group: "panels".into(),
                get_group: "main".into(),
                code: "area(i) > min_area".into(),
            }),
            Instruction::LoopOver(LoopOver {
                get_group: "panels".into(),
                iterator_name: "sign".into(),
                instructions: vec![Instruction::Filter(Filter {
                    set_group: "text".into(),
                    get_group: "main".into(),
                    code: "true".into(),
                })],
            }),
        ];

        let json = serde_json::to_string(&program).expect("should serialise");
        // Externally tagged, matching ivy's existing saved settings.
        assert!(json.contains(r#"{"RunCode":{"code""#), "got: {json}");
        assert!(json.contains(r#"{"Filter":{"set_group""#), "got: {json}");

        let back: Vec<Instruction> = serde_json::from_str(&json).expect("should deserialise");
        assert_eq!(back.len(), 3);
        assert_eq!(back[0].name(), "RunCode");
        assert_eq!(back[2].name(), "LoopOver");

        // The nested instructions survive, which is the part that needed
        // serde on LoopOver.
        match &back[2] {
            Instruction::LoopOver(l) => {
                assert_eq!(l.iterator_name, "sign");
                assert_eq!(l.instructions.len(), 1);
            }
            other => panic!("expected LoopOver, got {}", other.name()),
        }
    }
}
