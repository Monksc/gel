//! `SetOp` - algebra on group membership.
//!
//! Every other instruction writes its result by *replacing* a group, which
//! makes a `LoopOver` unable to accumulate: each iteration overwrites the
//! last. `SetOp`'s `append` is the missing primitive - it is what lets a
//! program collect per-sheet geometry into one group and `Emit` a single
//! combined file.
//!
//! This is membership algebra, not geometry: it moves shape indices between
//! groups and never touches a coordinate. For geometric set operations see
//! `Union` / `Intersect` / `Difference` in `geometry.rs`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::*;

/// Combines two groups' membership into a third.
///
/// A group that does not exist counts as empty rather than erroring, so an
/// accumulator needs no initialisation - the first `append` into a fresh
/// name just works.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetOp {
    /// Left operand. Absent = empty.
    pub get_group: String,
    /// Right operand. Absent = empty.
    pub with_group: String,
    pub set_group: String,
    /// `"append"` (default), `"union"`, `"intersect"`, `"difference"`.
    #[serde(default)]
    pub op: Option<String>,
}

impl Query for SetOp {
    fn query(&mut self, data: &mut Data) -> Result<(), String> {
        let get_group = resolve_template(data, &self.get_group, "SetOp.get_group")?;
        let with_group = resolve_template(data, &self.with_group, "SetOp.with_group")?;
        let set_group = resolve_template(data, &self.set_group, "SetOp.set_group")?;

        let mut groups = data.groups.lock().unwrap();
        let left = groups.get(&get_group).cloned().unwrap_or_default();
        let right = groups.get(&with_group).cloned().unwrap_or_default();

        let op = self.op.as_deref().unwrap_or("append");
        let result: Vec<Vec<usize>> = match op {
            // Slots are preserved, because slots are meaningful: `Nest`
            // writes one per sheet, and flattening them would lose which
            // part belongs to which sheet.
            "append" => left.into_iter().chain(right).collect(),

            // The rest are true set operations, so they flatten to one slot -
            // asking which slot an intersection belongs to has no answer.
            "union" | "intersect" | "difference" => {
                let l: BTreeSet<usize> = left.into_iter().flatten().collect();
                let r: BTreeSet<usize> = right.into_iter().flatten().collect();
                let merged: Vec<usize> = match op {
                    "union" => l.union(&r).copied().collect(),
                    "intersect" => l.intersection(&r).copied().collect(),
                    _ => l.difference(&r).copied().collect(),
                };
                if merged.is_empty() {
                    vec![]
                } else {
                    vec![merged]
                }
            }

            other => {
                return Err(format!(
                    "SetOp.op {other:?} is not one of append, union, intersect, difference"
                ))
            }
        };

        groups.insert(set_group, result);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::*;

    /// SetOp is pure membership algebra, so the shapes only need to exist -
    /// their geometry is never read, and `Data::from`'s reordering cannot
    /// affect the result.
    fn data_with(groups: &[(&str, Vec<Vec<usize>>)]) -> Data {
        use geo::{LineString, Polygon};
        let shapes: Vec<Polygon<f64>> = (0..4)
            .map(|_| {
                Polygon::new(
                    LineString::from(vec![(0.0_f64, 0.0_f64), (1.0, 0.0), (1.0, 1.0)]),
                    vec![],
                )
            })
            .collect();
        let data = Data::from(shapes);
        {
            let mut g = data.groups.lock().unwrap();
            for (name, slots) in groups {
                g.insert(name.to_string(), slots.clone());
            }
        }
        data
    }

    /// The accumulator pattern: appending into a group that does not exist
    /// yet must work, or every loop would need a priming step.
    #[test]
    fn append_accumulates_into_a_fresh_group() {
        let mut data = data_with(&[("a", vec![vec![0, 1]]), ("b", vec![vec![2]])]);

        SetOp {
            get_group: "acc".into(), // does not exist
            with_group: "a".into(),
            set_group: "acc".into(),
            op: None,
        }
        .query(&mut data)
        .expect("append into a missing group should succeed");

        SetOp {
            get_group: "acc".into(),
            with_group: "b".into(),
            set_group: "acc".into(),
            op: Some("append".into()),
        }
        .query(&mut data)
        .expect("second append should succeed");

        let groups = data.groups.lock().unwrap();
        // Slots stay separate - one per sheet - rather than flattening.
        assert_eq!(groups["acc"], vec![vec![0, 1], vec![2]]);
    }

    #[test]
    fn difference_and_intersect_flatten_to_one_slot() {
        let mut data = data_with(&[
            ("all", vec![vec![0, 1], vec![2, 3]]),
            ("tagged", vec![vec![1, 3]]),
        ]);

        SetOp {
            get_group: "all".into(),
            with_group: "tagged".into(),
            set_group: "rest".into(),
            op: Some("difference".into()),
        }
        .query(&mut data)
        .expect("difference should succeed");

        SetOp {
            get_group: "all".into(),
            with_group: "tagged".into(),
            set_group: "both".into(),
            op: Some("intersect".into()),
        }
        .query(&mut data)
        .expect("intersect should succeed");

        let groups = data.groups.lock().unwrap();
        assert_eq!(groups["rest"], vec![vec![0, 2]]);
        assert_eq!(groups["both"], vec![vec![1, 3]]);
    }

    #[test]
    fn an_unknown_op_is_an_error_rather_than_a_silent_append() {
        let mut data = data_with(&[]);
        let err = SetOp {
            get_group: "a".into(),
            with_group: "b".into(),
            set_group: "c".into(),
            op: Some("concat".into()),
        }
        .query(&mut data)
        .expect_err("a typo'd op must not quietly do something else");
        assert!(err.contains("concat"), "got: {err}");
    }
}
