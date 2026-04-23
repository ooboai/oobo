//! L2 — Turn store: DB-backed sink for the new capture pipeline.
//!
//! Responsibilities:
//!
//! 1. Accept [`Turn`](crate::core::turn::Turn) and
//!    [`SubagentLink`](crate::taps::SubagentLink) values from taps.
//! 2. Upsert them idempotently — same artifact re-ingested any number
//!    of times yields the same DB state.
//! 3. Never mutate `turns` rows once written, except via explicit
//!    `upsert` on the `(session_id, source, turn_index)` key.
//!
//! What this module DOES NOT do:
//! - Attribution: assigning turns to commits is the L3 pass
//!   ([`crate::attribution`]).
//! - Pricing: cost_usd may be populated by the caller, but the store
//!   does not look up rates.
//! - Subagent inference: the store writes the explicit parent/child
//!   relationship the tap told it about. Heuristic inference is M4
//!   and writes through this same store.

use rusqlite::{params, Connection};

use super::Db;
use crate::core::contribution::Contribution;
use crate::core::turn::{Turn, TurnRole};
use crate::taps::{SubagentLink, TurnSink};

/// Ensures the session row exists before turns are written.
///
/// The `sessions` PK is `(id, source)`. We use a minimal stub row
/// (name / model / created_at all NULL) so that FK constraints hold
/// during turn writes. The richer session metadata is filled in by
/// the existing session scanner on its next run; we never stomp on a
/// row that already exists.
pub fn ensure_session_stub(
    conn: &Connection,
    session_id: &str,
    source: &str,
    project_id: Option<&str>,
) -> Result<(), String> {
    // If no project id was passed in (common for turns discovered
    // outside any known project), fall back to a sentinel project so
    // the FK is satisfied. Projects table has a PK on id; we upsert
    // the sentinel.
    let project_id_final = project_id.unwrap_or("p:_unknown");
    conn.execute(
        "INSERT OR IGNORE INTO projects \
         (id, path, name, discovered_at, last_seen_at) \
         VALUES (?1, ?2, ?3, 0, 0)",
        params![
            project_id_final,
            project_id_final,
            project_id_final,
        ],
    )
    .map_err(|e| format!("ensure project sentinel: {e}"))?;

    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT OR IGNORE INTO sessions (id, source, project_id, indexed_at) \
         VALUES (?1, ?2, ?3, ?4)",
        params![session_id, source, project_id_final, now],
    )
    .map_err(|e| format!("ensure session stub: {e}"))?;
    Ok(())
}

/// Idempotent upsert of a single turn. Returns `true` if a new row
/// was inserted, `false` if an existing row was updated.
///
/// Upsert key is `(session_id, source, turn_index)` — the natural
/// identity of a turn. The generated `turns.id` column is kept in
/// sync with [`Turn::deterministic_id`].
pub fn upsert_turn(conn: &Connection, turn: &Turn) -> Result<bool, String> {
    let now = chrono::Utc::now().timestamp();
    // We want a real upsert, not INSERT OR IGNORE, because a later
    // re-ingest may have richer data (e.g. the tap learned a model
    // name or a cost computation filled cost_usd in post-processing).
    //
    // However, `turn_index` is part of the UNIQUE constraint but not
    // of PRIMARY KEY (which is `id`). ON CONFLICT needs the conflict
    // target; we handle that with ON CONFLICT(session_id, source,
    // turn_index) DO UPDATE so both reruns and id-stable writes
    // converge.
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM turns WHERE session_id=?1 AND source=?2 AND turn_index=?3",
            params![turn.session_id, turn.source, turn.turn_index],
            |r| r.get::<_, String>(0),
        )
        .ok();

    let changes = conn
        .execute(
            "INSERT INTO turns (\
                id, session_id, source, turn_index, role, \
                started_at, ended_at, model, \
                input_tokens, cache_read_tokens, cache_creation_tokens, output_tokens, \
                cost_usd, tool_call_count, thinking_ms, \
                message_preview, raw_ref, ingested_at\
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18) \
            ON CONFLICT(session_id, source, turn_index) DO UPDATE SET \
                role = excluded.role, \
                started_at = COALESCE(excluded.started_at, started_at), \
                ended_at = COALESCE(excluded.ended_at, ended_at), \
                model = COALESCE(excluded.model, model), \
                input_tokens = COALESCE(excluded.input_tokens, input_tokens), \
                cache_read_tokens = COALESCE(excluded.cache_read_tokens, cache_read_tokens), \
                cache_creation_tokens = COALESCE(excluded.cache_creation_tokens, cache_creation_tokens), \
                output_tokens = COALESCE(excluded.output_tokens, output_tokens), \
                cost_usd = COALESCE(excluded.cost_usd, cost_usd), \
                tool_call_count = MAX(tool_call_count, excluded.tool_call_count), \
                thinking_ms = COALESCE(excluded.thinking_ms, thinking_ms), \
                message_preview = COALESCE(excluded.message_preview, message_preview), \
                raw_ref = COALESCE(excluded.raw_ref, raw_ref), \
                ingested_at = excluded.ingested_at",
            params![
                turn.id,
                turn.session_id,
                turn.source,
                turn.turn_index,
                turn.role.as_str(),
                turn.started_at,
                turn.ended_at,
                turn.model,
                turn.tokens.input,
                turn.tokens.cache_read,
                turn.tokens.cache_creation,
                turn.tokens.output,
                turn.cost_usd,
                turn.tool_call_count,
                turn.thinking_ms,
                turn.message_preview,
                turn.raw_ref,
                now,
            ],
        )
        .map_err(|e| format!("upsert_turn: {e}"))?;

    Ok(existing.is_none() && changes > 0)
}

/// Write or update a parent/child link for a subagent session.
///
/// This writes onto the child session's row (`sessions.parent_*`
/// columns). Only fills columns the caller provided — the function
/// never clears a stronger previously-written link with weaker data.
pub fn upsert_subagent_link(conn: &Connection, link: &SubagentLink) -> Result<(), String> {
    ensure_session_stub(conn, &link.child_session_id, &link.child_source, None)?;
    conn.execute(
        "UPDATE sessions SET \
            parent_session_id = COALESCE(?1, parent_session_id), \
            parent_source     = COALESCE(?2, parent_source), \
            parent_turn_id    = COALESCE(?3, parent_turn_id), \
            subagent_kind     = COALESCE(?4, subagent_kind) \
         WHERE id = ?5 AND source = ?6",
        params![
            link.parent_session_id,
            link.parent_source,
            link.parent_turn_id,
            link.subagent_kind,
            link.child_session_id,
            link.child_source,
        ],
    )
    .map_err(|e| format!("upsert_subagent_link: {e}"))?;
    Ok(())
}

/// Write an anchor contribution row (delta window). Used by M5.
pub fn upsert_contribution(conn: &Connection, c: &Contribution) -> Result<(), String> {
    c.validate()?;
    conn.execute(
        "INSERT INTO anchor_contributions (\
            commit_hash, session_id, source, link_type, \
            first_turn_index, last_turn_index, \
            input_tokens, cache_read_tokens, cache_creation_tokens, output_tokens, \
            cost_usd, tool_call_count, duration_secs, \
            is_subagent, parent_session_id, parent_source, subagent_kind\
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17) \
         ON CONFLICT(commit_hash, session_id, source) DO UPDATE SET \
            link_type = excluded.link_type, \
            first_turn_index = excluded.first_turn_index, \
            last_turn_index  = excluded.last_turn_index, \
            input_tokens          = excluded.input_tokens, \
            cache_read_tokens     = excluded.cache_read_tokens, \
            cache_creation_tokens = excluded.cache_creation_tokens, \
            output_tokens         = excluded.output_tokens, \
            cost_usd        = excluded.cost_usd, \
            tool_call_count = excluded.tool_call_count, \
            duration_secs   = excluded.duration_secs, \
            is_subagent     = excluded.is_subagent, \
            parent_session_id = excluded.parent_session_id, \
            parent_source     = excluded.parent_source, \
            subagent_kind     = excluded.subagent_kind",
        params![
            c.commit_hash,
            c.session_id,
            c.source,
            c.link_type.as_str(),
            c.first_turn_index,
            c.last_turn_index,
            c.tokens.input,
            c.tokens.cache_read,
            c.tokens.cache_creation,
            c.tokens.output,
            c.cost_usd,
            c.tool_call_count,
            c.duration_secs,
            if c.is_subagent { 1 } else { 0 },
            c.parent_session_id,
            c.parent_source,
            c.subagent_kind,
        ],
    )
    .map_err(|e| format!("upsert_contribution: {e}"))?;
    Ok(())
}

/// Read a single turn's role, handy for tests / inference that need
/// to resolve a turn id back to its data.
pub fn fetch_turn_role(conn: &Connection, turn_id: &str) -> Option<TurnRole> {
    conn.query_row(
        "SELECT role FROM turns WHERE id = ?1",
        params![turn_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|s| TurnRole::parse(&s))
}

/// DB-backed [`TurnSink`]. Holds a mutable reference to the
/// connection so the sink can be scoped to a single tap run (and
/// optionally wrapped in a transaction by the caller). Transactions
/// are the caller's responsibility — see [`Db::ingest_in_tx`].
pub struct DbTurnSink<'a> {
    pub conn: &'a Connection,
    pub project_id: Option<String>,
    pub turns_written: u64,
    pub links_written: u64,
    pub errors: Vec<String>,
}

impl<'a> DbTurnSink<'a> {
    pub fn new(conn: &'a Connection, project_id: Option<String>) -> Self {
        Self {
            conn,
            project_id,
            turns_written: 0,
            links_written: 0,
            errors: Vec::new(),
        }
    }
}

impl<'a> TurnSink for DbTurnSink<'a> {
    fn accept_turn(&mut self, turn: Turn) {
        if let Err(e) =
            ensure_session_stub(self.conn, &turn.session_id, &turn.source, self.project_id.as_deref())
        {
            self.errors.push(format!("session stub: {e}"));
            return;
        }
        match upsert_turn(self.conn, &turn) {
            Ok(_) => self.turns_written += 1,
            Err(e) => self.errors.push(e),
        }
    }

    fn accept_subagent_link(&mut self, link: SubagentLink) {
        if let Err(e) = upsert_subagent_link(self.conn, &link) {
            self.errors.push(format!("subagent link: {e}"));
        } else {
            self.links_written += 1;
        }
    }
}

/// Convenience wrapper exposed on [`Db`] so callers don't touch
/// connections directly. Runs `f` inside a single transaction so
/// large ingestion jobs are atomic (and *vastly* faster because
/// SQLite's per-statement fsync is avoided).
impl Db {
    pub fn with_turn_sink<F, T>(&mut self, project_id: Option<String>, f: F) -> Result<T, String>
    where
        F: FnOnce(&mut DbTurnSink<'_>) -> T,
    {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin tx: {e}"))?;
        let mut sink = DbTurnSink::new(&tx, project_id);
        let out = f(&mut sink);
        if !sink.errors.is_empty() {
            let joined = sink.errors.join("; ");
            return Err(format!("turn sink errors: {joined}"));
        }
        tx.commit().map_err(|e| format!("commit tx: {e}"))?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::turn::{Turn, TurnRole, TurnTokens};

    fn sample_turn(session: &str, idx: i64, out: Option<i64>) -> Turn {
        Turn {
            id: Turn::deterministic_id("claude", session, idx),
            session_id: session.into(),
            source: "claude".into(),
            turn_index: idx,
            role: TurnRole::Assistant,
            started_at: Some(1_000 + idx),
            ended_at: Some(1_000 + idx),
            model: Some("claude-opus-4-5".into()),
            tokens: TurnTokens {
                output: out,
                ..Default::default()
            },
            cost_usd: None,
            tool_call_count: 0,
            thinking_ms: None,
            message_preview: Some(format!("msg-{idx}")),
            raw_ref: None,
        }
    }

    #[test]
    fn upsert_is_idempotent_on_key() {
        let mut db = Db::open_in_memory().unwrap();
        db.with_turn_sink(Some("r:t/p".into()), |sink| {
            sink.accept_turn(sample_turn("s1", 0, Some(50)));
            sink.accept_turn(sample_turn("s1", 0, Some(50)));
            sink.accept_turn(sample_turn("s1", 0, Some(50)));
        })
        .unwrap();

        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "same (session, source, turn_index) must collapse");

        let out: i64 = db
            .conn
            .query_row(
                "SELECT output_tokens FROM turns WHERE session_id='s1' AND turn_index=0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(out, 50);
    }

    #[test]
    fn re_ingest_fills_later_nulls_without_clobbering_earlier_values() {
        let mut db = Db::open_in_memory().unwrap();
        db.with_turn_sink(Some("r:t/p".into()), |sink| {
            // First pass: only model + output tokens known.
            sink.accept_turn(sample_turn("s1", 0, Some(10)));
        })
        .unwrap();

        // Second pass: richer turn with a cost computed.
        let mut later = sample_turn("s1", 0, Some(10));
        later.cost_usd = Some(0.42);
        later.tokens.cache_read = Some(2_000);

        db.with_turn_sink(Some("r:t/p".into()), |sink| {
            sink.accept_turn(later);
        })
        .unwrap();

        let (cost, cache_read, out, model): (Option<f64>, Option<i64>, Option<i64>, Option<String>) =
            db.conn
                .query_row(
                    "SELECT cost_usd, cache_read_tokens, output_tokens, model \
                     FROM turns WHERE session_id='s1' AND turn_index=0",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap();
        assert_eq!(cost, Some(0.42));
        assert_eq!(cache_read, Some(2_000));
        assert_eq!(out, Some(10), "existing value preserved through re-upsert");
        assert_eq!(model.as_deref(), Some("claude-opus-4-5"));
    }

    #[test]
    fn session_totals_view_equals_sum_of_turns() {
        let mut db = Db::open_in_memory().unwrap();
        db.with_turn_sink(Some("r:t/p".into()), |sink| {
            for i in 0..5 {
                sink.accept_turn(sample_turn("s1", i, Some(100)));
            }
        })
        .unwrap();

        let (turns, billed): (i64, i64) = db
            .conn
            .query_row(
                "SELECT turns, billed_tokens FROM v_session_totals \
                 WHERE session_id='s1' AND source='claude'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(turns, 5);
        assert_eq!(billed, 500, "5 turns × 100 output tokens each");
    }

    #[test]
    fn subagent_link_writes_parent_columns_on_child() {
        let mut db = Db::open_in_memory().unwrap();
        db.with_turn_sink(Some("r:t/p".into()), |sink| {
            sink.accept_turn(sample_turn("parent", 0, Some(10)));
            sink.accept_turn(sample_turn("child", 0, Some(5)));
            sink.accept_subagent_link(SubagentLink {
                child_session_id: "child".into(),
                child_source: "claude".into(),
                parent_session_id: "parent".into(),
                parent_source: "claude".into(),
                parent_turn_id: None,
                subagent_kind: Some("task".into()),
            });
        })
        .unwrap();

        let (pid, psrc, kind): (Option<String>, Option<String>, Option<String>) = db
            .conn
            .query_row(
                "SELECT parent_session_id, parent_source, subagent_kind \
                 FROM sessions WHERE id='child' AND source='claude'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(pid.as_deref(), Some("parent"));
        assert_eq!(psrc.as_deref(), Some("claude"));
        assert_eq!(kind.as_deref(), Some("task"));
    }

    #[test]
    fn contribution_upsert_then_view_matches() {
        let mut db = Db::open_in_memory().unwrap();
        // Seed 3 turns on one session.
        db.with_turn_sink(Some("r:t/p".into()), |sink| {
            for i in 0..3 {
                sink.accept_turn(sample_turn("s1", i, Some(100)));
            }
        })
        .unwrap();

        db.conn
            .execute(
                "INSERT INTO anchors (commit_hash, created_at) VALUES ('c1', 1)",
                [],
            )
            .unwrap();

        upsert_contribution(
            &db.conn,
            &Contribution {
                commit_hash: "c1".into(),
                session_id: "s1".into(),
                source: "claude".into(),
                link_type: crate::core::contribution::LinkType::Inferred,
                first_turn_index: 0,
                last_turn_index: 1,
                tokens: TurnTokens {
                    output: Some(200),
                    ..Default::default()
                },
                cost_usd: None,
                tool_call_count: Some(0),
                duration_secs: None,
                is_subagent: false,
                parent_session_id: None,
                parent_source: None,
                subagent_kind: None,
            },
        )
        .unwrap();

        let billed: i64 = db
            .conn
            .query_row(
                "SELECT billed_tokens FROM v_anchor_totals WHERE commit_hash='c1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(billed, 200, "contribution covers only turns 0..=1");

        // Re-upsert with a wider window and larger tokens; view must
        // reflect the new value, not sum them.
        upsert_contribution(
            &db.conn,
            &Contribution {
                commit_hash: "c1".into(),
                session_id: "s1".into(),
                source: "claude".into(),
                link_type: crate::core::contribution::LinkType::Explicit,
                first_turn_index: 0,
                last_turn_index: 2,
                tokens: TurnTokens {
                    output: Some(300),
                    ..Default::default()
                },
                cost_usd: None,
                tool_call_count: Some(0),
                duration_secs: None,
                is_subagent: false,
                parent_session_id: None,
                parent_source: None,
                subagent_kind: None,
            },
        )
        .unwrap();
        let billed2: i64 = db
            .conn
            .query_row(
                "SELECT billed_tokens FROM v_anchor_totals WHERE commit_hash='c1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(billed2, 300, "upsert replaces, never accumulates");
    }

    #[test]
    fn tx_wraps_sink_so_errors_roll_back() {
        let mut db = Db::open_in_memory().unwrap();
        let _ = db.with_turn_sink(Some("r:t/p".into()), |sink| {
            sink.accept_turn(sample_turn("s1", 0, Some(10)));
            sink.errors.push("simulated failure".into());
        });

        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "errored sink must roll back the tx");
    }
}
