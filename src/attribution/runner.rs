//! DB-backed runner for the [`compute_windows`](super::compute_windows)
//! pass. Reads `turns` + `anchors` for a project and writes
//! `anchor_contributions` via the L2 store.
//!
//! Invocation contexts:
//!
//! - **Backfill**: after a bulk turn-ingest run (re-reading all Claude
//!   transcripts), call [`rebuild_contributions_for_project`] once
//!   to recompute from scratch.
//! - **Incremental commit hook**: after a new anchor is written, call
//!   [`append_contributions_since_last_anchor`] (TODO: impl in M5b)
//!   to attribute only the new turns without touching prior rows.
//!
//! The runner is deliberately thin — all interesting logic lives in
//! the pure [`compute_windows`] function upstream.

use rusqlite::params;

use super::{compute_windows, AttrAnchor, AttrTurn, PriorCursor};
use crate::core::contribution::LinkType;
use crate::core::turn::TurnTokens;
use crate::db::turns::upsert_contribution;
use crate::db::Db;

/// Recompute all contributions for a project from scratch.
///
/// Strategy:
/// 1. Delete existing contributions for this project's anchors
///    (cheap, few rows per commit).
/// 2. Load all anchors for the project (via `anchors` table + `ai_commits`
///    pivot on project_id since `anchors` doesn't carry project_id yet).
/// 3. Load all turns for sessions in this project.
/// 4. Run [`compute_windows`] and upsert.
///
/// Return value: number of contribution rows written.
pub fn rebuild_contributions_for_project(
    db: &Db,
    project_id: &str,
) -> Result<usize, String> {
    let anchors = load_project_anchors(db, project_id)?;
    let turns = load_project_turns(db, project_id)?;

    if anchors.is_empty() || turns.is_empty() {
        return Ok(0);
    }

    // Wipe the slate for this project's anchors so we never leave
    // stale contributions from a prior run of an earlier algorithm.
    let anchor_placeholders = anchors.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "DELETE FROM anchor_contributions WHERE commit_hash IN ({anchor_placeholders})"
    );
    let commit_params: Vec<&dyn rusqlite::ToSql> = anchors
        .iter()
        .map(|a| &a.commit_hash as &dyn rusqlite::ToSql)
        .collect();
    db.conn
        .execute(&sql, &commit_params[..])
        .map_err(|e| format!("purge prior contributions: {e}"))?;

    let contributions = compute_windows(&anchors, &turns, PriorCursor::new());

    // Stamp explicit link_type on any contribution whose session was
    // explicitly linked by a hook (old `anchor_sessions` with
    // `link_type='explicit'`). Cheap migration path from the old
    // table: we don't need to rewrite the algorithm; we just decorate
    // its inferred output with the stronger provenance when we have it.
    let explicit_pairs = load_explicit_links(db, project_id)?;

    let tx = db
        .conn
        .unchecked_transaction()
        .map_err(|e| format!("begin tx: {e}"))?;
    let mut n = 0usize;
    for mut c in contributions {
        if explicit_pairs.contains(&(c.commit_hash.clone(), c.session_id.clone(), c.source.clone())) {
            c.link_type = LinkType::Explicit;
        }
        // Inherit subagent metadata from the session row if present.
        if let Some((parent_sid, parent_src, kind)) =
            load_session_parent(&tx, &c.session_id, &c.source)?
        {
            c.is_subagent = true;
            c.parent_session_id = Some(parent_sid);
            c.parent_source = Some(parent_src);
            c.subagent_kind = kind;
        }
        upsert_contribution(&tx, &c)?;
        n += 1;
    }
    tx.commit().map_err(|e| format!("commit tx: {e}"))?;

    Ok(n)
}

fn load_project_anchors(db: &Db, project_id: &str) -> Result<Vec<AttrAnchor>, String> {
    // Anchors aren't directly keyed by project_id in the current
    // schema; we project through ai_commits which carries project_id.
    // Anchors that have no ai_commits entry (because they predate
    // that table's population) fall back to a LEFT JOIN ... IS NULL
    // heuristic — but for attribution correctness we only want
    // project-scoped anchors, so we keep it tight.
    let mut stmt = db
        .conn
        .prepare(
            "SELECT DISTINCT a.commit_hash, a.committed_at \
             FROM anchors a \
             JOIN ai_commits c ON c.commit_hash = a.commit_hash \
             WHERE c.project_id = ?1 \
             ORDER BY a.committed_at ASC",
        )
        .map_err(|e| format!("prepare anchors: {e}"))?;
    let rows = stmt
        .query_map(params![project_id], |r| {
            Ok(AttrAnchor {
                commit_hash: r.get::<_, String>(0)?,
                committed_at_ms: r.get::<_, Option<i64>>(1)?.unwrap_or(0) * 1000,
            })
        })
        .map_err(|e| format!("query anchors: {e}"))?;

    crate::db::collect_rows(rows)
}

fn load_project_turns(db: &Db, project_id: &str) -> Result<Vec<AttrTurn>, String> {
    let mut stmt = db
        .conn
        .prepare(
            "SELECT t.session_id, t.source, t.turn_index, t.started_at, \
                    t.input_tokens, t.cache_read_tokens, t.cache_creation_tokens, t.output_tokens, \
                    t.cost_usd, t.tool_call_count, \
                    (t.ended_at - t.started_at) AS duration_ms \
             FROM turns t \
             JOIN sessions s ON s.id = t.session_id AND s.source = t.source \
             WHERE s.project_id = ?1 \
             ORDER BY t.session_id, t.source, t.turn_index ASC",
        )
        .map_err(|e| format!("prepare turns: {e}"))?;
    let rows = stmt
        .query_map(params![project_id], |r| {
            Ok(AttrTurn {
                session_id: r.get::<_, String>(0)?,
                source: r.get::<_, String>(1)?,
                turn_index: r.get::<_, i64>(2)?,
                started_at: r.get::<_, Option<i64>>(3)?,
                tokens: TurnTokens {
                    input: r.get::<_, Option<i64>>(4)?,
                    cache_read: r.get::<_, Option<i64>>(5)?,
                    cache_creation: r.get::<_, Option<i64>>(6)?,
                    output: r.get::<_, Option<i64>>(7)?,
                },
                cost_usd: r.get::<_, Option<f64>>(8)?,
                tool_call_count: r.get::<_, Option<i64>>(9)?.unwrap_or(0),
                duration_ms: r.get::<_, Option<i64>>(10)?,
            })
        })
        .map_err(|e| format!("query turns: {e}"))?;

    crate::db::collect_rows(rows)
}

/// Explicit commit↔session pairs promoted from the legacy
/// `anchor_sessions` table. Returns a set of `(commit, session, source)`
/// triples with `link_type = 'explicit'`.
fn load_explicit_links(
    db: &Db,
    project_id: &str,
) -> Result<std::collections::HashSet<(String, String, String)>, String> {
    let mut stmt = db
        .conn
        .prepare(
            "SELECT asx.commit_hash, asx.session_id, asx.agent \
             FROM anchor_sessions asx \
             JOIN ai_commits c ON c.commit_hash = asx.commit_hash \
             WHERE c.project_id = ?1 AND asx.link_type = 'explicit'",
        )
        .map_err(|e| format!("prepare explicit links: {e}"))?;
    let rows = stmt
        .query_map(params![project_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| format!("query explicit links: {e}"))?;

    let mut out = std::collections::HashSet::new();
    for t in rows.flatten() {
        out.insert(t);
    }
    Ok(out)
}

fn load_session_parent(
    conn: &rusqlite::Connection,
    session_id: &str,
    source: &str,
) -> Result<Option<(String, String, Option<String>)>, String> {
    let row = conn
        .query_row(
            "SELECT parent_session_id, parent_source, subagent_kind \
             FROM sessions WHERE id = ?1 AND source = ?2",
            params![session_id, source],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .ok();
    Ok(match row {
        Some((Some(pid), Some(psrc), kind)) => Some((pid, psrc, kind)),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::turn::{Turn, TurnRole};
    use crate::taps::TurnSink;
    use rusqlite::params;

    fn mk_turn(session: &str, idx: i64, ts_ms: i64, out: i64) -> Turn {
        Turn {
            id: Turn::deterministic_id("claude", session, idx),
            session_id: session.into(),
            source: "claude".into(),
            turn_index: idx,
            role: TurnRole::Assistant,
            started_at: Some(ts_ms),
            ended_at: Some(ts_ms),
            model: None,
            tokens: TurnTokens {
                output: Some(out),
                ..Default::default()
            },
            cost_usd: None,
            tool_call_count: 0,
            thinking_ms: None,
            message_preview: None,
            raw_ref: None,
        }
    }

    fn seed_project(db: &Db, pid: &str) {
        db.conn
            .execute(
                "INSERT INTO projects (id, path, name, discovered_at, last_seen_at) \
                 VALUES (?1, ?2, ?3, 1, 1)",
                params![pid, pid, pid],
            )
            .unwrap();
    }

    fn seed_anchor(db: &Db, project_id: &str, commit: &str, committed_at_secs: i64) {
        db.conn
            .execute(
                "INSERT INTO anchors (commit_hash, committed_at, created_at) \
                 VALUES (?1, ?2, ?2)",
                params![commit, committed_at_secs],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO ai_commits (commit_hash, branch_name, project_id, source, ingested_at) \
                 VALUES (?1, 'main', ?2, 'cursor', 1)",
                params![commit, project_id],
            )
            .unwrap();
    }

    #[test]
    fn rebuild_produces_correct_per_commit_totals() {
        let mut db = Db::open_in_memory().unwrap();
        seed_project(&db, "r:gh/acme/p");

        db.with_turn_sink(Some("r:gh/acme/p".into()), |sink| {
            // 100ms per turn, 100 output tokens each.
            sink.accept_turn(mk_turn("s1", 0, 100, 100));
            sink.accept_turn(mk_turn("s1", 1, 200, 100));
            sink.accept_turn(mk_turn("s1", 2, 400, 100));
            sink.accept_turn(mk_turn("s1", 3, 500, 100));
        })
        .unwrap();

        // committed_at is in seconds per schema; compute_windows
        // converts by *1000. Anchor at 0.3s covers turns 0..1, at 0.6s
        // covers 2..3.
        seed_anchor(&db, "r:gh/acme/p", "c1", 0); // unix t=0 → ms=0 → covers nothing
        // Override c1 to 0.3s (300 ms).
        db.conn
            .execute(
                "UPDATE anchors SET committed_at = ?1 WHERE commit_hash='c1'",
                params![0i64], // anchors.committed_at is secs; can't store fractional. Use ceil.
            )
            .unwrap();
        // Use integer seconds: c1 at 1s (1000ms), c2 at 1s as well won't work.
        // Use 1 and 2 seconds instead; adjust turn timestamps accordingly.
        db.conn
            .execute("DELETE FROM turns", [])
            .unwrap();
        db.with_turn_sink(Some("r:gh/acme/p".into()), |sink| {
            // ts in ms: 500, 800, 1500, 1800
            sink.accept_turn(mk_turn("s1", 0, 500, 100));
            sink.accept_turn(mk_turn("s1", 1, 800, 100));
            sink.accept_turn(mk_turn("s1", 2, 1500, 100));
            sink.accept_turn(mk_turn("s1", 3, 1800, 100));
        })
        .unwrap();
        db.conn
            .execute(
                "UPDATE anchors SET committed_at = 1 WHERE commit_hash='c1'",
                [],
            )
            .unwrap();
        seed_anchor(&db, "r:gh/acme/p", "c2", 2);

        let n = rebuild_contributions_for_project(&db, "r:gh/acme/p").unwrap();
        assert_eq!(n, 2, "one contribution per commit for this session");

        let (c1_out, c1_first, c1_last): (i64, i64, i64) = db
            .conn
            .query_row(
                "SELECT output_tokens, first_turn_index, last_turn_index \
                 FROM anchor_contributions WHERE commit_hash='c1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((c1_first, c1_last), (0, 1));
        assert_eq!(c1_out, 200);

        let (c2_out, c2_first, c2_last): (i64, i64, i64) = db
            .conn
            .query_row(
                "SELECT output_tokens, first_turn_index, last_turn_index \
                 FROM anchor_contributions WHERE commit_hash='c2'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((c2_first, c2_last), (2, 3));
        assert_eq!(c2_out, 200);

        // v_anchor_totals view agrees.
        let total_via_view: i64 = db
            .conn
            .query_row(
                "SELECT SUM(billed_tokens) FROM v_anchor_totals WHERE commit_hash IN ('c1','c2')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(total_via_view, 400);

        // Project view agrees.
        let project_total: i64 = db
            .conn
            .query_row(
                "SELECT billed_tokens FROM v_project_totals WHERE project_id='r:gh/acme/p'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(project_total, 400);
    }

    #[test]
    fn rebuild_is_idempotent() {
        let mut db = Db::open_in_memory().unwrap();
        seed_project(&db, "r:gh/acme/p");
        db.with_turn_sink(Some("r:gh/acme/p".into()), |sink| {
            sink.accept_turn(mk_turn("s1", 0, 100, 10));
            sink.accept_turn(mk_turn("s1", 1, 200, 20));
        })
        .unwrap();
        seed_anchor(&db, "r:gh/acme/p", "c1", 1);

        let first = rebuild_contributions_for_project(&db, "r:gh/acme/p").unwrap();
        let second = rebuild_contributions_for_project(&db, "r:gh/acme/p").unwrap();
        assert_eq!(first, second);

        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM anchor_contributions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "no duplicate rows after rerun");
    }

    #[test]
    fn explicit_link_upgrades_link_type() {
        let mut db = Db::open_in_memory().unwrap();
        seed_project(&db, "r:gh/acme/p");
        db.with_turn_sink(Some("r:gh/acme/p".into()), |sink| {
            sink.accept_turn(mk_turn("s1", 0, 100, 10));
        })
        .unwrap();
        seed_anchor(&db, "r:gh/acme/p", "c1", 1);
        db.conn
            .execute(
                "INSERT INTO anchor_sessions (commit_hash, session_id, agent, link_type) \
                 VALUES ('c1', 's1', 'claude', 'explicit')",
                [],
            )
            .unwrap();

        rebuild_contributions_for_project(&db, "r:gh/acme/p").unwrap();

        let lt: String = db
            .conn
            .query_row(
                "SELECT link_type FROM anchor_contributions WHERE commit_hash='c1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(lt, "explicit");
    }

    #[test]
    fn subagent_contribution_inherits_parent_from_sessions() {
        let mut db = Db::open_in_memory().unwrap();
        seed_project(&db, "r:gh/acme/p");
        db.with_turn_sink(Some("r:gh/acme/p".into()), |sink| {
            sink.accept_turn(mk_turn("parent", 0, 100, 10));
            sink.accept_turn(mk_turn("child", 0, 200, 5));
            sink.accept_subagent_link(crate::taps::SubagentLink {
                child_session_id: "child".into(),
                child_source: "claude".into(),
                parent_session_id: "parent".into(),
                parent_source: "claude".into(),
                parent_turn_id: None,
                subagent_kind: Some("task".into()),
            });
        })
        .unwrap();
        seed_anchor(&db, "r:gh/acme/p", "c1", 1);

        rebuild_contributions_for_project(&db, "r:gh/acme/p").unwrap();

        let (is_sub, kind): (i64, Option<String>) = db
            .conn
            .query_row(
                "SELECT is_subagent, subagent_kind FROM anchor_contributions \
                 WHERE commit_hash='c1' AND session_id='child'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(is_sub, 1);
        assert_eq!(kind.as_deref(), Some("task"));

        // v_anchor_totals should split sub vs top-level.
        let (top, sub): (i64, i64) = db
            .conn
            .query_row(
                "SELECT top_level_sessions, subagents FROM v_anchor_totals WHERE commit_hash='c1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((top, sub), (1, 1));
    }
}
