//! L3 — Attribution: assign session turn windows to anchors.
//!
//! # The core question
//!
//! > For a given commit, which turns from which sessions should count
//! > as "work done for this commit"?
//!
//! The old schema answered this by storing cumulative per-session
//! totals on each commit, which multiplied the same turns across N
//! commits. The new schema answers it by storing a DELTA window
//! `[first_turn_index, last_turn_index]` per (commit, session) and
//! forbidding overlap across commits within a single session.
//!
//! # The attribution algorithm
//!
//! Chronological walk over anchors in a project. For each anchor `A`
//! at time `t_A`, for each session `S` with at least one turn:
//!
//! 1. Let `prev_last` = the largest `last_turn_index` previously
//!    attributed to `S` across any prior anchor (or `-1` if none).
//! 2. Let `window_end` = the largest `turn_index` in `S` whose
//!    `started_at <= t_A`.
//! 3. If `window_end <= prev_last`, nothing new to attribute. Skip.
//! 4. Otherwise write contribution `[prev_last + 1, window_end]` with
//!    token sums taken directly from the `turns` table rows in that
//!    half-open range.
//!
//! This pass is idempotent: running it twice on the same state
//! produces the same rows.
//!
//! # Linking policy
//!
//! - An explicit (hook-provided) commit→session link is honored
//!   exactly as given, with `link_type = explicit`.
//! - Everything else is `link_type = inferred` (time-based).
//!
//! # What this file does NOT do
//!
//! - Open a DB connection. Attribution is a pure function over plain
//!   data (see [`compute_windows`]) so it's trivially testable.
//! - Decide subagent hierarchy. Contributions inherit
//!   `is_subagent` / `parent_session_id` from the `sessions` table;
//!   that's M4's job.

#[cfg(test)]
pub mod inference;
pub mod turn_store;

use crate::core::contribution::{Contribution, LinkType};
use crate::core::turn::TurnTokens;

/// A single turn as the attributor needs to see it. Ordering across
/// `(session_id, source)` groups is defined by `turn_index`.
#[derive(Debug, Clone, PartialEq)]
pub struct AttrTurn {
    pub session_id: String,
    pub source: String,
    pub turn_index: i64,
    /// Milliseconds since epoch.
    pub started_at: Option<i64>,
    pub tokens: TurnTokens,
    pub cost_usd: Option<f64>,
    pub tool_call_count: i64,
    pub duration_ms: Option<i64>,
}

/// A single anchor, in chronological order by `committed_at`.
#[derive(Debug, Clone, PartialEq)]
pub struct AttrAnchor {
    pub commit_hash: String,
    /// Milliseconds since epoch. Anchors are iterated in ascending
    /// order of this value.
    pub committed_at_ms: i64,
}

/// Prior state the attributor needs to know so it can resume from
/// where a previous run left off. Maps `(session_id, source)` to the
/// largest `last_turn_index` already attributed to any anchor.
pub type PriorCursor = std::collections::HashMap<(String, String), i64>;

/// Pure-function attribution. Returns the contributions that SHOULD
/// exist in the database for these anchors given these turns and this
/// prior cursor, in the order the DB should write them in.
///
/// Contract:
/// - `anchors` MUST be sorted ascending by `committed_at_ms`.
/// - `turns` MUST be grouped by (session_id, source) and sorted
///   ascending by `turn_index` within each group.
/// - The function does not mutate its inputs.
///
/// The return value is a flat `Vec<Contribution>` in anchor order
/// then session order, so the caller can persist it with a single
/// streaming loop.
pub fn compute_windows(
    anchors: &[AttrAnchor],
    turns: &[AttrTurn],
    mut cursor: PriorCursor,
) -> Vec<Contribution> {
    let mut out: Vec<Contribution> = Vec::new();

    // Group turns per session for fast window scans. Each vec retains
    // its input order, which the contract states is turn_index ASC.
    use std::collections::BTreeMap;
    let mut by_session: BTreeMap<(String, String), Vec<&AttrTurn>> = BTreeMap::new();
    for t in turns {
        by_session
            .entry((t.session_id.clone(), t.source.clone()))
            .or_default()
            .push(t);
    }

    for anchor in anchors {
        for ((session_id, source), session_turns) in &by_session {
            let prev_last = *cursor
                .get(&(session_id.clone(), source.clone()))
                .unwrap_or(&-1);

            // Find the largest turn_index whose started_at <= anchor.committed_at_ms.
            // Turns with no timestamp are considered "before" any
            // anchor — this is the behavior the store's backfill
            // path expects (timestamps are optional per schema).
            let mut window_end: i64 = -1;
            for t in session_turns {
                let eligible = match t.started_at {
                    Some(ts) => ts <= anchor.committed_at_ms,
                    None => true,
                };
                if eligible && t.turn_index > window_end {
                    window_end = t.turn_index;
                }
            }

            if window_end <= prev_last {
                continue;
            }

            let first = prev_last + 1;
            let last = window_end;

            let mut tokens = TurnTokens::default();
            let mut cost_usd_acc: Option<f64> = None;
            let mut tool_calls: i64 = 0;
            let mut duration_ms_acc: i64 = 0;
            let mut has_duration = false;

            for t in session_turns {
                if t.turn_index < first || t.turn_index > last {
                    continue;
                }
                if let Some(v) = t.tokens.input {
                    tokens.input = Some(tokens.input.unwrap_or(0) + v);
                }
                if let Some(v) = t.tokens.cache_read {
                    tokens.cache_read = Some(tokens.cache_read.unwrap_or(0) + v);
                }
                if let Some(v) = t.tokens.cache_creation {
                    tokens.cache_creation = Some(tokens.cache_creation.unwrap_or(0) + v);
                }
                if let Some(v) = t.tokens.output {
                    tokens.output = Some(tokens.output.unwrap_or(0) + v);
                }
                if let Some(c) = t.cost_usd {
                    cost_usd_acc = Some(cost_usd_acc.unwrap_or(0.0) + c);
                }
                tool_calls += t.tool_call_count;
                if let Some(d) = t.duration_ms {
                    duration_ms_acc += d;
                    has_duration = true;
                }
            }

            out.push(Contribution {
                commit_hash: anchor.commit_hash.clone(),
                session_id: session_id.clone(),
                source: source.clone(),
                link_type: LinkType::Inferred,
                first_turn_index: first,
                last_turn_index: last,
                tokens,
                cost_usd: cost_usd_acc,
                tool_call_count: Some(tool_calls),
                duration_secs: if has_duration {
                    Some((duration_ms_acc / 1000).max(0))
                } else {
                    None
                },
                is_subagent: false,
                parent_session_id: None,
                parent_source: None,
                subagent_kind: None,
            });

            cursor.insert((session_id.clone(), source.clone()), last);
        }
    }

    out
}

pub mod runner;

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(session: &str, idx: i64, ts: Option<i64>, out: i64) -> AttrTurn {
        AttrTurn {
            session_id: session.into(),
            source: "claude".into(),
            turn_index: idx,
            started_at: ts,
            tokens: TurnTokens {
                output: Some(out),
                ..Default::default()
            },
            cost_usd: None,
            tool_call_count: 0,
            duration_ms: None,
        }
    }

    #[test]
    fn single_session_single_anchor_captures_all_prior_turns() {
        let turns = vec![
            turn("s1", 0, Some(100), 10),
            turn("s1", 1, Some(200), 20),
            turn("s1", 2, Some(300), 30),
        ];
        let anchors = vec![AttrAnchor {
            commit_hash: "c1".into(),
            committed_at_ms: 400,
        }];
        let out = compute_windows(&anchors, &turns, PriorCursor::new());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].first_turn_index, 0);
        assert_eq!(out[0].last_turn_index, 2);
        assert_eq!(out[0].tokens.output, Some(60));
    }

    #[test]
    fn multiple_anchors_partition_turns_without_overlap() {
        let turns = vec![
            turn("s1", 0, Some(100), 10),
            turn("s1", 1, Some(200), 20),
            turn("s1", 2, Some(400), 30),
            turn("s1", 3, Some(500), 40),
        ];
        let anchors = vec![
            AttrAnchor {
                commit_hash: "c1".into(),
                committed_at_ms: 300,
            },
            AttrAnchor {
                commit_hash: "c2".into(),
                committed_at_ms: 600,
            },
        ];
        let out = compute_windows(&anchors, &turns, PriorCursor::new());
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].commit_hash, "c1");
        assert_eq!(out[0].first_turn_index, 0);
        assert_eq!(out[0].last_turn_index, 1);
        assert_eq!(out[0].tokens.output, Some(30));

        assert_eq!(out[1].commit_hash, "c2");
        assert_eq!(out[1].first_turn_index, 2);
        assert_eq!(out[1].last_turn_index, 3);
        assert_eq!(out[1].tokens.output, Some(70));

        // Sum of contributions == total turn tokens (the core invariant).
        let total: i64 = out.iter().map(|c| c.tokens.output.unwrap_or(0)).sum();
        assert_eq!(total, 100);
    }

    #[test]
    fn resumes_from_prior_cursor() {
        let turns = vec![
            turn("s1", 0, Some(100), 10),
            turn("s1", 1, Some(200), 20),
            turn("s1", 2, Some(300), 30),
        ];
        let anchors = vec![AttrAnchor {
            commit_hash: "c2".into(),
            committed_at_ms: 400,
        }];
        let mut cursor = PriorCursor::new();
        cursor.insert(("s1".into(), "claude".into()), 0);

        let out = compute_windows(&anchors, &turns, cursor);
        assert_eq!(out.len(), 1);
        // Prior anchor attributed up to idx=0; new window starts at 1.
        assert_eq!(out[0].first_turn_index, 1);
        assert_eq!(out[0].last_turn_index, 2);
        assert_eq!(out[0].tokens.output, Some(50));
    }

    #[test]
    fn sessions_newer_than_first_anchor_skipped() {
        let turns = vec![turn("s1", 0, Some(100), 10), turn("s2", 0, Some(500), 99)];
        let anchors = vec![AttrAnchor {
            commit_hash: "c1".into(),
            committed_at_ms: 300,
        }];
        let out = compute_windows(&anchors, &turns, PriorCursor::new());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].session_id, "s1");
    }

    #[test]
    fn turns_with_no_timestamp_go_to_first_available_anchor() {
        // Backfill case: turns arrive without timestamps from a tool
        // that doesn't expose them. They should be attributed to the
        // earliest anchor so the data isn't left orphaned.
        let turns = vec![turn("s1", 0, None, 7), turn("s1", 1, None, 3)];
        let anchors = vec![
            AttrAnchor {
                commit_hash: "c1".into(),
                committed_at_ms: 100,
            },
            AttrAnchor {
                commit_hash: "c2".into(),
                committed_at_ms: 200,
            },
        ];
        let out = compute_windows(&anchors, &turns, PriorCursor::new());
        assert_eq!(out.len(), 1, "c2 has nothing left after c1");
        assert_eq!(out[0].commit_hash, "c1");
        assert_eq!(out[0].tokens.output, Some(10));
    }

    #[test]
    fn no_double_count_across_many_commits_and_sessions() {
        // Two sessions, three commits, interleaved turns. Final
        // invariant: SUM(contributions) == SUM(turns) per session.
        let turns = vec![
            turn("s1", 0, Some(100), 10),
            turn("s2", 0, Some(150), 20),
            turn("s1", 1, Some(250), 30),
            turn("s2", 1, Some(350), 40),
            turn("s1", 2, Some(550), 50),
        ];
        let anchors = vec![
            AttrAnchor {
                commit_hash: "c1".into(),
                committed_at_ms: 200,
            },
            AttrAnchor {
                commit_hash: "c2".into(),
                committed_at_ms: 400,
            },
            AttrAnchor {
                commit_hash: "c3".into(),
                committed_at_ms: 600,
            },
        ];
        let contribs = compute_windows(&anchors, &turns, PriorCursor::new());

        let s1_total: i64 = contribs
            .iter()
            .filter(|c| c.session_id == "s1")
            .map(|c| c.tokens.output.unwrap_or(0))
            .sum();
        let s2_total: i64 = contribs
            .iter()
            .filter(|c| c.session_id == "s2")
            .map(|c| c.tokens.output.unwrap_or(0))
            .sum();
        assert_eq!(s1_total, 10 + 30 + 50);
        assert_eq!(s2_total, 20 + 40);

        // No window overlaps within a session.
        let mut s1_windows: Vec<(i64, i64)> = contribs
            .iter()
            .filter(|c| c.session_id == "s1")
            .map(|c| (c.first_turn_index, c.last_turn_index))
            .collect();
        s1_windows.sort_unstable();
        for pair in s1_windows.windows(2) {
            assert!(pair[0].1 < pair[1].0, "overlap detected: {pair:?}");
        }
    }

    #[test]
    fn token_sums_across_all_four_components() {
        let mut t = turn("s1", 0, Some(100), 0);
        t.tokens = TurnTokens {
            input: Some(10),
            cache_read: Some(100),
            cache_creation: Some(5),
            output: Some(40),
        };
        let anchors = vec![AttrAnchor {
            commit_hash: "c1".into(),
            committed_at_ms: 200,
        }];
        let out = compute_windows(&anchors, &[t], PriorCursor::new());
        assert_eq!(out[0].tokens.billed(), 155);
    }

    #[test]
    fn tool_calls_and_cost_sum_correctly_across_window() {
        let mut t1 = turn("s1", 0, Some(100), 10);
        t1.tool_call_count = 2;
        t1.cost_usd = Some(0.01);
        let mut t2 = turn("s1", 1, Some(200), 20);
        t2.tool_call_count = 3;
        t2.cost_usd = Some(0.02);

        let anchors = vec![AttrAnchor {
            commit_hash: "c1".into(),
            committed_at_ms: 300,
        }];
        let out = compute_windows(&anchors, &[t1, t2], PriorCursor::new());
        assert_eq!(out[0].tool_call_count, Some(5));
        assert!((out[0].cost_usd.unwrap() - 0.03).abs() < 1e-9);
    }

    #[test]
    fn idempotent_when_rerun_on_same_inputs() {
        let turns = vec![turn("s1", 0, Some(100), 10), turn("s1", 1, Some(200), 20)];
        let anchors = vec![AttrAnchor {
            commit_hash: "c1".into(),
            committed_at_ms: 300,
        }];
        let r1 = compute_windows(&anchors, &turns, PriorCursor::new());
        let r2 = compute_windows(&anchors, &turns, PriorCursor::new());
        assert_eq!(r1, r2);
    }
}
