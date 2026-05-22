//! DB adapter for the pure [`inference`](super::inference) engine.
//!
//! Loads orphan child sessions + Task-tool parent candidates for a
//! project, runs [`infer`], writes the decisions to the audit table,
//! and applies the ones whose score clears [`APPLY_THRESHOLD`] back
//! onto the `sessions` row.
//!
//! This module is kept deliberately thin — the heuristics live in
//! [`super::inference`] so they can be tested without touching SQL.

use rusqlite::params;

use super::inference::{infer, OrphanChild, ParentTurn};
use crate::db::Db;

/// Outcome of one inference pass for a project.
#[derive(Debug, Clone, Default)]
pub struct InferenceReport {
    pub orphans_considered: usize,
    pub parents_considered: usize,
    pub proposed: usize,
    pub applied: usize,
}

/// Run inference for a single project. Idempotent: repeated runs
/// converge (applied links are not re-written, rejected ones produce
/// new audit rows with fresh `decided_at`).
pub fn infer_subagents_for_project(
    db: &Db,
    project_id: &str,
) -> Result<InferenceReport, String> {
    let orphans = load_orphan_children(db, project_id)?;
    let parents = load_task_tool_parents(db, project_id)?;

    let mut report = InferenceReport {
        orphans_considered: orphans.len(),
        parents_considered: parents.len(),
        ..Default::default()
    };

    if orphans.is_empty() || parents.is_empty() {
        return Ok(report);
    }

    let inferences = infer(&orphans, &parents);
    report.proposed = inferences.len();

    let now_ms = chrono::Utc::now().timestamp_millis();

    let tx = db
        .conn
        .unchecked_transaction()
        .map_err(|e| format!("begin tx: {e}"))?;

    for inf in &inferences {
        let applied = inf.should_apply();

        // Audit row: always written, regardless of whether we apply.
        tx.execute(
            "INSERT OR IGNORE INTO subagent_inferences (\
                child_session_id, child_source, \
                parent_session_id, parent_source, parent_turn_id, \
                subagent_kind, score, signals_json, applied, decided_at\
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                inf.child_session_id,
                inf.child_source,
                inf.parent_session_id,
                inf.parent_source,
                inf.parent_turn_id,
                inf.subagent_kind,
                inf.score as f64,
                inf.signals_json(),
                if applied { 1 } else { 0 },
                now_ms,
            ],
        )
        .map_err(|e| format!("audit row: {e}"))?;

        if !applied {
            continue;
        }

        // Apply: only fill columns that are still NULL on the child.
        // This preserves any explicit link written by a tap — the
        // hard "never overwrite explicit" invariant.
        let changed = tx
            .execute(
                "UPDATE sessions SET \
                    parent_session_id = COALESCE(parent_session_id, ?1), \
                    parent_source     = COALESCE(parent_source,     ?2), \
                    parent_turn_id    = COALESCE(parent_turn_id,    ?3), \
                    subagent_kind     = COALESCE(subagent_kind,     ?4) \
                 WHERE id = ?5 AND source = ?6 \
                   AND parent_session_id IS NULL",
                params![
                    inf.parent_session_id,
                    inf.parent_source,
                    inf.parent_turn_id,
                    inf.subagent_kind,
                    inf.child_session_id,
                    inf.child_source,
                ],
            )
            .map_err(|e| format!("apply link: {e}"))?;

        if changed > 0 {
            report.applied += 1;
        }
    }

    tx.commit().map_err(|e| format!("commit: {e}"))?;
    Ok(report)
}

/// Orphans: sessions in the project with no parent link and at
/// least one turn. Their `first_user_preview` comes from the
/// earliest user turn, which is the best template-preamble signal.
fn load_orphan_children(
    db: &Db,
    project_id: &str,
) -> Result<Vec<OrphanChild>, String> {
    let mut stmt = db
        .conn
        .prepare(
            "SELECT s.id, s.source, \
                    (SELECT MIN(t.started_at) \
                       FROM turns t \
                      WHERE t.session_id = s.id AND t.source = s.source) AS first_ts, \
                    (SELECT t2.message_preview \
                       FROM turns t2 \
                      WHERE t2.session_id = s.id AND t2.source = s.source \
                        AND t2.role = 'user' \
                      ORDER BY t2.turn_index ASC LIMIT 1) AS first_user_preview \
             FROM sessions s \
             WHERE s.project_id = ?1 \
               AND s.parent_session_id IS NULL \
               AND EXISTS (SELECT 1 FROM turns t \
                            WHERE t.session_id = s.id AND t.source = s.source)",
        )
        .map_err(|e| format!("prepare orphans: {e}"))?;

    let rows = stmt
        .query_map(params![project_id], |r| {
            Ok(OrphanChild {
                session_id: r.get(0)?,
                source: r.get(1)?,
                first_turn_started_at_ms: r.get::<_, Option<i64>>(2)?,
                first_user_preview: r.get::<_, Option<String>>(3)?,
            })
        })
        .map_err(|e| format!("query orphans: {e}"))?;

    crate::db::collect_rows(rows)
}

/// Parent candidates: every assistant turn in the project whose
/// `tool_names` contains `Task`. The v12 column makes this O(rows)
/// instead of "re-parse the JSONL."
fn load_task_tool_parents(
    db: &Db,
    project_id: &str,
) -> Result<Vec<ParentTurn>, String> {
    let mut stmt = db
        .conn
        .prepare(
            "SELECT t.id, t.session_id, t.source, t.turn_index, t.started_at, t.tool_names \
             FROM turns t \
             JOIN sessions s ON s.id = t.session_id AND s.source = t.source \
             WHERE s.project_id = ?1 \
               AND t.role = 'assistant' \
               AND t.tool_names IS NOT NULL \
               AND (',' || t.tool_names || ',') LIKE '%,Task,%'",
        )
        .map_err(|e| format!("prepare parents: {e}"))?;

    let rows = stmt
        .query_map(params![project_id], |r| {
            Ok(ParentTurn {
                turn_id: r.get(0)?,
                session_id: r.get(1)?,
                source: r.get(2)?,
                turn_index: r.get(3)?,
                started_at_ms: r.get::<_, Option<i64>>(4)?,
                tool_names: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
            })
        })
        .map_err(|e| format!("query parents: {e}"))?;

    crate::db::collect_rows(rows)
}

#[cfg(test)]
#[allow(unused_imports)]
mod tests {
    use super::*;
    use crate::core::turn::{Turn, TurnRole, TurnTokens};
    use crate::taps::TurnSink;

    fn seed_project(db: &Db, pid: &str) {
        db.conn
            .execute(
                "INSERT OR IGNORE INTO projects (id, path, name, discovered_at, last_seen_at) \
                 VALUES (?1, ?1, ?1, 0, 0)",
                params![pid],
            )
            .unwrap();
    }

    fn seed_session(db: &Db, pid: &str, sid: &str) {
        seed_project(db, pid);
        db.conn
            .execute(
                "INSERT OR IGNORE INTO sessions (id, source, project_id, indexed_at) \
                 VALUES (?1, 'claude', ?2, 0)",
                params![sid, pid],
            )
            .unwrap();
    }

    fn mk_turn(
        sid: &str,
        idx: i64,
        ts_ms: i64,
        role: TurnRole,
        tool_names: Option<&str>,
        preview: Option<&str>,
    ) -> Turn {
        Turn {
            id: Turn::deterministic_id("claude", sid, idx),
            session_id: sid.into(),
            source: "claude".into(),
            turn_index: idx,
            role,
            started_at: Some(ts_ms),
            ended_at: Some(ts_ms),
            model: None,
            tokens: TurnTokens::default(),
            cost_usd: None,
            tool_call_count: if tool_names.is_some() { 1 } else { 0 },
            thinking_ms: None,
            message_preview: preview.map(String::from),
            raw_ref: None,
            tool_names: tool_names.map(String::from),
        }
    }

    #[test]
    fn end_to_end_links_orphan_child_when_task_tool_fired_moments_before() {
        let mut db = Db::open_in_memory().unwrap();
        let pid = "r:t/p";

        // Parent session: one assistant turn with Task tool_use at t=1_000_000.
        seed_session(&db, pid, "parent-1");
        // Child session: first turn at t=1_000_500 (500ms after parent).
        seed_session(&db, pid, "child-1");

        db.with_turn_sink(Some(pid.into()), |sink| {
            sink.accept_turn(mk_turn(
                "parent-1",
                5,
                1_000_000,
                TurnRole::Assistant,
                Some("Task"),
                Some("I'll launch a subagent"),
            ));
            sink.accept_turn(mk_turn(
                "child-1",
                0,
                1_000_500,
                TurnRole::User,
                None,
                Some("You are a task-focused agent. Go find X."),
            ));
            sink.accept_turn(mk_turn(
                "child-1",
                1,
                1_000_600,
                TurnRole::Assistant,
                None,
                Some("found X"),
            ));
        })
        .unwrap();

        let report = infer_subagents_for_project(&db, pid).unwrap();
        assert_eq!(report.orphans_considered, 2,
            "both sessions look like orphans to the loader (parent has no parent either)");
        assert_eq!(report.parents_considered, 1);
        assert_eq!(report.proposed, 1, "only child-1 can pair with parent-1");
        assert_eq!(report.applied, 1);

        // Session row was updated.
        let (pid_row, kind): (Option<String>, Option<String>) = db
            .conn
            .query_row(
                "SELECT parent_session_id, subagent_kind \
                 FROM sessions WHERE id='child-1' AND source='claude'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(pid_row.as_deref(), Some("parent-1"));
        assert_eq!(kind.as_deref(), Some("task"));

        // Audit row was written.
        let n: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM subagent_inferences WHERE child_session_id='child-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn never_overwrites_explicit_parent_link() {
        let mut db = Db::open_in_memory().unwrap();
        let pid = "r:t/p";
        seed_session(&db, pid, "parent-1");
        seed_session(&db, pid, "other-parent");
        seed_session(&db, pid, "child-1");

        // Pre-existing explicit link via the sessions row.
        db.conn
            .execute(
                "UPDATE sessions SET parent_session_id='other-parent', \
                                     parent_source='claude', \
                                     subagent_kind='task' \
                 WHERE id='child-1'",
                [],
            )
            .unwrap();

        db.with_turn_sink(Some(pid.into()), |sink| {
            sink.accept_turn(mk_turn(
                "parent-1",
                0,
                1_000_000,
                TurnRole::Assistant,
                Some("Task"),
                None,
            ));
            sink.accept_turn(mk_turn(
                "child-1",
                0,
                1_000_500,
                TurnRole::User,
                None,
                Some("You are a task-focused agent"),
            ));
        })
        .unwrap();

        let report = infer_subagents_for_project(&db, pid).unwrap();
        // Orphans excludes child-1 (already linked) AND other-parent
        // (no turns), leaving only parent-1 — which is itself an
        // orphan here but has no Task-using parent candidate.
        assert_eq!(report.orphans_considered, 1);
        assert_eq!(report.applied, 0);

        let (p_id,): (String,) = db
            .conn
            .query_row(
                "SELECT parent_session_id FROM sessions WHERE id='child-1'",
                [],
                |r| Ok((r.get(0)?,)),
            )
            .unwrap();
        assert_eq!(p_id, "other-parent", "explicit link untouched");
    }

    #[test]
    fn no_task_tool_parents_means_no_inferences() {
        let mut db = Db::open_in_memory().unwrap();
        let pid = "r:t/p";
        seed_session(&db, pid, "s1");
        seed_session(&db, pid, "s2");

        db.with_turn_sink(Some(pid.into()), |sink| {
            sink.accept_turn(mk_turn(
                "s1",
                0,
                1_000_000,
                TurnRole::Assistant,
                Some("Read,Write"),
                None,
            ));
            sink.accept_turn(mk_turn(
                "s2",
                0,
                1_000_500,
                TurnRole::User,
                None,
                None,
            ));
        })
        .unwrap();

        let r = infer_subagents_for_project(&db, pid).unwrap();
        assert_eq!(r.parents_considered, 0);
        assert_eq!(r.proposed, 0);
        assert_eq!(r.applied, 0);
    }
}
