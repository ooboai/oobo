//! M4 — Subagent inference engine.
//!
//! # What this module answers
//!
//! > When the tool doesn't say "this session is a subagent of that
//! > one," can we figure it out from evidence?
//!
//! The explicit path is already covered by taps that see a
//! filesystem convention (Claude's `subagents/<id>.jsonl`) and emit
//! [`SubagentLink`](crate::taps::SubagentLink) values onto the
//! sessions table. This module closes the gap for everything else:
//! renamed artifacts, pre-convention transcripts, and tools that
//! never expose the link at all.
//!
//! # Design: multi-signal scoring, never overwrite explicit
//!
//! Each evaluation takes an **orphan child** — a session with
//! `parent_session_id IS NULL` — and scores it against every
//! **parent candidate** (an assistant turn that invoked the `Task`
//! tool). Each heuristic contributes a weighted [`SignalHit`]; the
//! total score is capped at `1.0`. We apply a link iff
//! `score >= APPLY_THRESHOLD` (0.6 by default).
//!
//! Signals, strongest first:
//!
//! | Signal                | Weight | Purpose                                |
//! |-----------------------|--------|----------------------------------------|
//! | `TaskToolTemporal`    | 0.7    | Parent Task tool fired moments before  |
//! |                       |        | the orphan's first turn.               |
//! | `TemplatePreamble`    | 0.4    | Child's first user message starts with |
//! |                       |        | a known subagent launch template.      |
//!
//! Signals are intentionally additive: a child that matches
//! **temporal** (0.7) **and** **template** (0.4) scores above the
//! cap at 1.0; either alone at 0.6 would also suffice, but we
//! require `TaskToolTemporal` as a hard precondition because
//! temporal coincidence is what turns "similar-looking template"
//! from coincidence into causation.
//!
//! # Determinism, audit, idempotency
//!
//! - Same inputs → same outputs. No wall-clock decisions; uses
//!   monotonic timestamps from the data only.
//! - Every decision (applied or not) writes a row to
//!   `subagent_inferences` keyed by `(child_session_id, child_source,
//!   decided_at)`, with the full signal mix serialized as JSON.
//! - Re-running the engine never downgrades an existing explicit
//!   link. It only fills `parent_session_id` columns that are still
//!   `NULL`.
//!
//! # Why this is kept pure
//!
//! [`infer`] is a pure function over plain structs. The DB layer is
//! a thin adapter that loads the inputs, calls `infer`, then writes
//! the outputs. This makes the heuristic easy to test with hand-
//! crafted fixtures (see the `#[cfg(test)]` section), easy to
//! replay, and safe to iterate on.

use serde::Serialize;

/// How close (in ms) a child's first turn must start to a parent's
/// Task tool call to count as a temporal match.
///
/// Tuned from observation: Claude's Task-tool subagent launches
/// appear on disk within ~5s of the tool_use. We allow a generous
/// window on both sides to survive clock skew between transcript
/// files and a long parent turn that holds the tool_use near its
/// end.
const TEMPORAL_WINDOW_MS_BEFORE: i64 = 10_000;
const TEMPORAL_WINDOW_MS_AFTER: i64 = 60_000;

/// Min combined score required to write the link back onto the
/// sessions row. Borderline decisions are still audited but not
/// applied.
pub const APPLY_THRESHOLD: f32 = 0.6;

/// Well-known preambles that subagent templates put at the top of
/// their first user message. Presence counts as medium evidence.
const TEMPLATE_PREAMBLES: &[(&str, &str)] = &[
    // (substring_match, inferred_subagent_kind)
    ("You are a task-focused agent", "task"),
    ("You are being launched as a subagent", "task"),
    ("You are a subagent", "task"),
    ("<subagent_prompt>", "task"),
];

/// An assistant turn that invoked the `Task` tool. Candidate parent
/// for any orphan child whose first turn lands in its temporal
/// window.
#[derive(Debug, Clone)]
pub struct ParentTurn {
    pub session_id: String,
    pub source: String,
    pub turn_id: String,
    #[allow(dead_code)]
    pub turn_index: i64,
    /// Milliseconds since epoch. `None` means the candidate has no
    /// timestamp, which disqualifies it from temporal scoring.
    pub started_at_ms: Option<i64>,
    /// Comma-joined tool names seen on this turn. Must contain
    /// "Task" for a candidate to be passed in.
    pub tool_names: String,
}

/// A session that currently has no parent link. Candidate child for
/// any parent whose Task tool fired close to this session's first
/// turn.
#[derive(Debug, Clone)]
pub struct OrphanChild {
    pub session_id: String,
    pub source: String,
    /// Timestamp (ms) of the child's first recorded turn. Required
    /// for temporal scoring.
    pub first_turn_started_at_ms: Option<i64>,
    /// Redacted preview of the child's first user message, used for
    /// template-preamble matching.
    pub first_user_preview: Option<String>,
}

/// One heuristic hit contributing to an inference's total score.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SignalHit {
    /// Parent Task tool_use fired within the temporal window before
    /// the child's first turn.
    TaskToolTemporal {
        weight: f32,
        /// Gap from parent turn start to child first turn (ms).
        /// Negative values mean the child started before the parent
        /// turn's timestamp — still counted inside a tolerance.
        gap_ms: i64,
    },
    /// Child's first user message starts with a well-known subagent
    /// template preamble.
    TemplatePreamble {
        weight: f32,
        /// The preamble substring that matched.
        matched: String,
        /// The subagent kind implied by the preamble
        /// (e.g. `"task"`).
        inferred_kind: String,
    },
}

impl SignalHit {
    pub fn weight(&self) -> f32 {
        match self {
            SignalHit::TaskToolTemporal { weight, .. } => *weight,
            SignalHit::TemplatePreamble { weight, .. } => *weight,
        }
    }
}

/// A scored parent→child proposal. Even when `score <
/// APPLY_THRESHOLD`, the engine emits the record so the `subagent_
/// inferences` audit table captures the reasoning.
#[derive(Debug, Clone)]
pub struct Inference {
    pub child_session_id: String,
    pub child_source: String,
    pub parent_session_id: String,
    pub parent_source: String,
    pub parent_turn_id: Option<String>,
    pub subagent_kind: Option<String>,
    pub score: f32,
    pub signals: Vec<SignalHit>,
}

impl Inference {
    /// Whether the link should be written back to the sessions row.
    pub fn should_apply(&self) -> bool {
        self.score >= APPLY_THRESHOLD
    }

    /// Stable JSON encoding of the signals for the audit row.
    pub fn signals_json(&self) -> String {
        serde_json::to_string(&self.signals).unwrap_or_else(|_| "[]".to_string())
    }
}

/// Core inference pass (pure function).
///
/// For each orphan in [`orphans`], finds the best-scoring parent
/// candidate in [`parents`] and emits one [`Inference`] per orphan
/// that produced *any* positive signal. Orphans with no signal hits
/// at all are dropped (we don't audit empty evaluations).
///
/// Ordering:
/// - Input slices can be in any order.
/// - Output is ordered by `child_session_id` then `score` desc, so
///   two runs over the same data produce byte-identical audit logs.
pub fn infer(orphans: &[OrphanChild], parents: &[ParentTurn]) -> Vec<Inference> {
    let mut out: Vec<Inference> = Vec::with_capacity(orphans.len());

    for child in orphans {
        // Template preamble is a one-shot per child: precompute once.
        let template_hit = detect_template_preamble(child.first_user_preview.as_deref());

        let mut best: Option<Inference> = None;

        for parent in parents {
            // A candidate only counts if its turn names include
            // "Task" — this is the hard precondition the DB query
            // already enforces but we re-assert in the pure fn.
            if !tool_names_contains(&parent.tool_names, "Task") {
                continue;
            }
            // Don't self-link.
            if parent.session_id == child.session_id && parent.source == child.source {
                continue;
            }

            let temporal = temporal_match(parent, child);
            // Without a temporal match we refuse to propose anything:
            // template alone is too lossy in the wild.
            let temporal = match temporal {
                Some(s) => s,
                None => continue,
            };

            let mut signals: Vec<SignalHit> = vec![temporal];
            let mut subagent_kind: Option<String> = None;

            if let Some((kind, hit)) = &template_hit {
                signals.push(hit.clone());
                subagent_kind = Some(kind.clone());
            }

            let score = signals.iter().map(|s| s.weight()).sum::<f32>().min(1.0);

            let inference = Inference {
                child_session_id: child.session_id.clone(),
                child_source: child.source.clone(),
                parent_session_id: parent.session_id.clone(),
                parent_source: parent.source.clone(),
                parent_turn_id: Some(parent.turn_id.clone()),
                subagent_kind: subagent_kind.or(Some("task".into())),
                score,
                signals,
            };

            // Tie-breaking (strict order so the result is
            // permutation-invariant):
            //   1. Higher score wins.
            //   2. Smaller absolute temporal gap wins (closer is better).
            //   3. Lexicographically smaller parent session id wins.
            //   4. Smaller parent turn_index wins.
            best = Some(match best.take() {
                None => inference,
                Some(prev) => {
                    if inference.score > prev.score {
                        inference
                    } else if inference.score < prev.score {
                        prev
                    } else {
                        let prev_gap = abs_gap(&prev);
                        let new_gap = abs_gap(&inference);
                        if new_gap < prev_gap {
                            inference
                        } else if new_gap > prev_gap {
                            prev
                        } else if inference.parent_session_id < prev.parent_session_id {
                            inference
                        } else if inference.parent_session_id > prev.parent_session_id {
                            prev
                        } else {
                            // Final tie-break by parent turn id
                            // (which encodes turn_index).
                            let new_tid = inference.parent_turn_id.as_deref().unwrap_or("");
                            let prev_tid = prev.parent_turn_id.as_deref().unwrap_or("");
                            if new_tid < prev_tid {
                                inference
                            } else {
                                prev
                            }
                        }
                    }
                }
            });
        }

        if let Some(b) = best {
            out.push(b);
        }
    }

    out.sort_by(|a, b| {
        a.child_session_id.cmp(&b.child_session_id).then(
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    out
}

/// Temporal scoring. Returns `Some(hit)` when the child's first turn
/// starts within `[parent_ts - TEMPORAL_WINDOW_MS_BEFORE, parent_ts
/// + TEMPORAL_WINDOW_MS_AFTER]`. Earlier-is-harder: the closer the
///   child starts after the parent (the expected order), the stronger
///   the signal, up to the full weight.
fn temporal_match(parent: &ParentTurn, child: &OrphanChild) -> Option<SignalHit> {
    let pts = parent.started_at_ms?;
    let cts = child.first_turn_started_at_ms?;
    let gap = cts - pts;

    if !(-TEMPORAL_WINDOW_MS_BEFORE..=TEMPORAL_WINDOW_MS_AFTER).contains(&gap) {
        return None;
    }

    // Weight falloff: full 0.7 if 0..=5s after parent, linear to
    // 0.5 at the boundaries. This prefers the "parent fired, child
    // started within seconds" case that dominates real data.
    let weight = if (0..=5_000).contains(&gap) {
        0.7
    } else {
        let span = (TEMPORAL_WINDOW_MS_AFTER.max(TEMPORAL_WINDOW_MS_BEFORE)) as f32;
        let dist = gap.unsigned_abs() as f32;
        let decay = (1.0 - (dist / span)).clamp(0.0, 1.0);
        (0.5 + 0.2 * decay).min(0.7)
    };

    Some(SignalHit::TaskToolTemporal {
        weight,
        gap_ms: gap,
    })
}

/// Detect a known subagent template preamble in the child's first
/// user message. Returns `(subagent_kind, hit)` when one matches.
fn detect_template_preamble(preview: Option<&str>) -> Option<(String, SignalHit)> {
    let preview = preview?.trim_start();
    for (needle, kind) in TEMPLATE_PREAMBLES {
        if preview.starts_with(needle)
            || preview.contains(needle) && starts_within_head(preview, needle, 256)
        {
            return Some((
                (*kind).into(),
                SignalHit::TemplatePreamble {
                    weight: 0.4,
                    matched: (*needle).into(),
                    inferred_kind: (*kind).into(),
                },
            ));
        }
    }
    None
}

/// Absolute temporal gap from the inference's signals, or `i64::MAX`
/// when no temporal signal is present.
fn abs_gap(inf: &Inference) -> i64 {
    inf.signals
        .iter()
        .find_map(|s| match s {
            SignalHit::TaskToolTemporal { gap_ms, .. } => Some(gap_ms.abs()),
            _ => None,
        })
        .unwrap_or(i64::MAX)
}

/// `true` if `needle` first occurs within the first `head_bytes`
/// bytes of `haystack`. Keeps the template check tight — a template
/// wouldn't be buried deep in a user message.
fn starts_within_head(haystack: &str, needle: &str, head_bytes: usize) -> bool {
    let head_end = haystack.len().min(head_bytes);
    haystack[..head_end].contains(needle)
}

/// Comma-split tool_names membership check. Anchored on exact
/// elements so `"TaskRunner"` does not match `"Task"`.
fn tool_names_contains(names: &str, target: &str) -> bool {
    names.split(',').any(|n| n.trim() == target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parent(session: &str, turn_idx: i64, ts_ms: i64, tools: &str) -> ParentTurn {
        ParentTurn {
            session_id: session.into(),
            source: "claude".into(),
            turn_id: format!("t-{session}-{turn_idx}"),
            turn_index: turn_idx,
            started_at_ms: Some(ts_ms),
            tool_names: tools.into(),
        }
    }

    fn orphan(session: &str, first_ts_ms: Option<i64>, preview: Option<&str>) -> OrphanChild {
        OrphanChild {
            session_id: session.into(),
            source: "claude".into(),
            first_turn_started_at_ms: first_ts_ms,
            first_user_preview: preview.map(String::from),
        }
    }

    #[test]
    fn temporal_match_within_5s_gets_full_weight() {
        let p = parent("P", 3, 1_000_000, "Task");
        let c = orphan("C", Some(1_000_500), None);
        let hit = temporal_match(&p, &c).expect("within window");
        match hit {
            SignalHit::TaskToolTemporal { weight, gap_ms } => {
                assert_eq!(gap_ms, 500);
                assert_eq!(weight, 0.7);
            }
            _ => panic!("wrong signal kind"),
        }
    }

    #[test]
    fn temporal_match_outside_window_rejects() {
        let p = parent("P", 3, 1_000_000, "Task");
        // child starts a minute and 10s later — outside AFTER window
        let c = orphan("C", Some(1_000_000 + 70_000), None);
        assert!(temporal_match(&p, &c).is_none());
    }

    #[test]
    fn temporal_match_slightly_before_parent_is_still_allowed() {
        let p = parent("P", 3, 1_000_000, "Task");
        let c = orphan("C", Some(1_000_000 - 3_000), None);
        assert!(
            temporal_match(&p, &c).is_some(),
            "clock skew / same-turn boundary must still match"
        );
    }

    #[test]
    fn template_preamble_detected_at_start() {
        let hit =
            detect_template_preamble(Some("You are a task-focused agent. Do X and report back."));
        assert!(hit.is_some());
        let (kind, sig) = hit.unwrap();
        assert_eq!(kind, "task");
        match sig {
            SignalHit::TemplatePreamble {
                weight,
                inferred_kind,
                ..
            } => {
                assert_eq!(weight, 0.4);
                assert_eq!(inferred_kind, "task");
            }
            _ => panic!("wrong signal kind"),
        }
    }

    #[test]
    fn template_preamble_rejects_random_text() {
        assert!(detect_template_preamble(Some("Hi, how are you?")).is_none());
        assert!(detect_template_preamble(None).is_none());
    }

    #[test]
    fn tool_names_match_is_exact_on_comma_split() {
        assert!(tool_names_contains("Read,Task,Write", "Task"));
        assert!(tool_names_contains("Task", "Task"));
        assert!(!tool_names_contains("TaskRunner", "Task"));
        assert!(!tool_names_contains("Read,Write", "Task"));
    }

    #[test]
    fn infer_returns_no_result_when_no_parent_uses_task_tool() {
        let parents = vec![parent("P", 0, 1_000_000, "Read,Write")];
        let orphans = vec![orphan("C", Some(1_000_500), None)];
        assert!(infer(&orphans, &parents).is_empty());
    }

    #[test]
    fn infer_applies_strongly_on_temporal_plus_template() {
        let parents = vec![parent("P", 5, 1_000_000, "Task")];
        let orphans = vec![orphan(
            "C",
            Some(1_000_500),
            Some("You are a task-focused agent performing exploration."),
        )];
        let out = infer(&orphans, &parents);
        assert_eq!(out.len(), 1);
        let inf = &out[0];
        assert_eq!(inf.parent_session_id, "P");
        assert_eq!(inf.parent_turn_id.as_deref(), Some("t-P-5"));
        assert_eq!(inf.subagent_kind.as_deref(), Some("task"));
        assert_eq!(inf.signals.len(), 2);
        assert!(inf.should_apply(), "0.7 + 0.4 capped at 1.0 must apply");
        assert!(inf.score >= APPLY_THRESHOLD);
    }

    #[test]
    fn infer_applies_on_temporal_alone_at_full_weight() {
        // Temporal alone at 0.7 exceeds the 0.6 threshold — this is
        // the common case for subagent detection without template
        // preambles in the preview.
        let parents = vec![parent("P", 5, 1_000_000, "Task")];
        let orphans = vec![orphan("C", Some(1_000_500), Some("go find foo"))];
        let out = infer(&orphans, &parents);
        assert_eq!(out.len(), 1);
        assert!(out[0].should_apply());
        assert_eq!(out[0].signals.len(), 1);
    }

    #[test]
    fn infer_picks_best_parent_when_multiple_candidates() {
        let parents = vec![
            parent("P1", 0, 900_000, "Task"), // ~100s before child → outside window
            parent("P2", 3, 1_000_000, "Task"), // 500ms before child → full weight
            parent("P3", 7, 1_020_000, "Task"), // 20s after child → in window but lower weight
        ];
        let orphans = vec![orphan("C", Some(1_000_500), None)];
        let out = infer(&orphans, &parents);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].parent_session_id, "P2",
            "closest before-child match wins on weight AND gap tie-breaker"
        );
    }

    #[test]
    fn infer_never_links_session_to_itself() {
        let parents = vec![parent("S", 0, 1_000_000, "Task")];
        let orphans = vec![orphan("S", Some(1_000_500), None)];
        assert!(infer(&orphans, &parents).is_empty());
    }

    #[test]
    fn audit_signals_json_round_trips() {
        let parents = vec![parent("P", 0, 1_000_000, "Task")];
        let orphans = vec![orphan("C", Some(1_000_500), Some("You are a subagent"))];
        let out = infer(&orphans, &parents);
        let json = out[0].signals_json();
        // Stable shape: array of {"kind": ..., ...}.
        assert!(json.starts_with("[{"));
        assert!(json.contains("task_tool_temporal"));
        assert!(json.contains("template_preamble"));
        // Re-parse for good measure.
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v.as_array().unwrap().len() == 2);
    }

    #[test]
    fn infer_is_deterministic_under_input_permutation() {
        let mut parents = vec![
            parent("P1", 0, 1_000_000, "Task"),
            parent("P2", 0, 1_000_100, "Task"),
        ];
        let mut orphans = vec![
            orphan("B", Some(1_000_200), None),
            orphan("A", Some(1_000_200), None),
        ];
        let out1 = infer(&orphans, &parents);

        parents.reverse();
        orphans.reverse();
        let out2 = infer(&orphans, &parents);

        assert_eq!(out1.len(), out2.len());
        for (a, b) in out1.iter().zip(out2.iter()) {
            assert_eq!(a.child_session_id, b.child_session_id);
            assert_eq!(a.parent_session_id, b.parent_session_id);
        }
    }
}
