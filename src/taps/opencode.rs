//! OpenCode L1 tap.
//!
//! OpenCode stores sessions in `~/Library/Application Support/opencode/opencode.db`
//! (macOS) with two schema generations:
//!
//! - **Modern (v1.2+)**: one row per message in `message`, with
//!   `data` as JSON containing `{role, modelID, tokens:{input,
//!   output, cache:{read,write}}, finish, ...}`. `part` rows hold
//!   individual content pieces (text / tool calls / tool results).
//!   These are **per-call** tokens — exactly what we want.
//! - **Legacy (pre-v1.2)**: session-level aggregates only. No per-turn
//!   deltas, so this tap emits a single synthetic assistant turn
//!   carrying the rolled-up tokens rather than fabricating ones that
//!   don't exist.
//!
//! Subagent hierarchy: modern schema stores `session.parent_id`
//! natively. The tap does **not** emit those links here — they're
//! already captured during session discovery. Keeping this tap
//! narrowly focused on turn emission (the hierarchy is attached
//! session-level, which the store's `sessions` upsert owns).

use std::path::Path;

use super::{Source, TapArtifact, TapError, TapSummary, TurnSink, TurnTap};
use crate::config::Config;
use crate::core::turn::{Turn, TurnRole, TurnTokens};

pub const SOURCE: Source = "opencode";

pub struct OpenCodeTurnTap;

impl TurnTap for OpenCodeTurnTap {
    fn source(&self) -> Source {
        SOURCE
    }

    fn enabled(&self, cfg: &Config) -> bool {
        cfg.opencode.enabled
    }

    fn ingest_session(
        &self,
        session_id: &str,
        artifact: TapArtifact<'_>,
        sink: &mut dyn TurnSink,
    ) -> Result<TapSummary, TapError> {
        let path = match artifact {
            TapArtifact::File(p) => p,
            _ => {
                return Err(TapError::Other(
                    "opencode tap only supports TapArtifact::File (the opencode.db path)".into(),
                ))
            }
        };
        ingest_db(session_id, path, sink)
    }
}

fn ingest_db(
    session_id: &str,
    db_path: &Path,
    sink: &mut dyn TurnSink,
) -> Result<TapSummary, TapError> {
    let conn = crate::utils::open_db_readonly(db_path)
        .map_err(|e| TapError::Other(format!("opencode db open failed: {e}")))?;

    if is_modern(&conn) {
        ingest_modern(&conn, session_id, sink)
    } else {
        ingest_legacy(&conn, session_id, sink)
    }
}

fn is_modern(conn: &rusqlite::Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='project'",
        [],
        |row| row.get(0),
    )
    .unwrap_or(false)
}

fn ingest_modern(
    conn: &rusqlite::Connection,
    session_id: &str,
    sink: &mut dyn TurnSink,
) -> Result<TapSummary, TapError> {
    let mut summary = TapSummary::default();

    // Gather all parts for this session up-front, grouped by
    // message_id. One SQL roundtrip; O(total parts) memory — modest
    // given one session's transcript typically fits comfortably.
    let tool_names_by_message = load_tool_names_per_message(conn, session_id);
    let text_by_message = load_text_per_message(conn, session_id);

    let mut stmt = conn
        .prepare(
            "SELECT id, time_created, data \
             FROM message \
             WHERE session_id = ?1 \
             ORDER BY time_created ASC, id ASC",
        )
        .map_err(|e| TapError::Other(format!("opencode messages query: {e}")))?;

    let rows = stmt
        .query_map([session_id], |row| {
            let id: String = row.get(0)?;
            let ts: i64 = row.get(1)?;
            let data: String = row.get(2)?;
            Ok((id, ts, data))
        })
        .map_err(|e| TapError::Other(format!("opencode messages rows: {e}")))?;

    let mut turn_index: i64 = 0;

    for row in rows.flatten() {
        let (msg_id, ts, data_str) = row;
        let v: serde_json::Value = match serde_json::from_str(&data_str) {
            Ok(v) => v,
            Err(_) => {
                summary.turns_skipped += 1;
                continue;
            }
        };

        let role_str = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
        let role = match role_str {
            "user" => TurnRole::User,
            "assistant" => TurnRole::Assistant,
            // OpenCode also uses 'tool' for tool results — we fold
            // those into the assistant turn's tool metadata via
            // part lookup, so skip here.
            _ => {
                summary.turns_skipped += 1;
                continue;
            }
        };

        let model = v
            .get("modelID")
            .and_then(|m| m.as_str())
            .map(|s| s.to_string());

        let tokens = extract_modern_tokens(&v);
        let cost_usd = v.get("cost").and_then(|c| c.as_f64());

        let tool_names = tool_names_by_message
            .get(&msg_id)
            .map(|names| names.join(","));
        let tool_call_count = tool_names_by_message
            .get(&msg_id)
            .map(|n| n.len() as i64)
            .unwrap_or(0);

        let preview = text_by_message.get(&msg_id).map(|t| clip_preview(t));

        let ts_ms = normalize_ts(ts);

        let turn = Turn {
            id: Turn::deterministic_id(SOURCE, session_id, turn_index),
            session_id: session_id.to_string(),
            source: SOURCE.to_string(),
            turn_index,
            role,
            started_at: Some(ts_ms),
            ended_at: Some(ts_ms),
            model,
            tokens,
            cost_usd,
            tool_call_count,
            thinking_ms: None,
            message_preview: preview,
            raw_ref: Some(format!("opencode:{session_id}/{msg_id}")),
            tool_names,
        };

        sink.accept_turn(turn);
        summary.turns_emitted += 1;
        turn_index += 1;
    }

    if summary.turns_emitted == 0 {
        summary
            .warnings
            .push(format!("opencode: no messages for session {session_id}"));
    }

    Ok(summary)
}

fn extract_modern_tokens(v: &serde_json::Value) -> TurnTokens {
    let tokens = match v.get("tokens") {
        Some(t) => t,
        None => return TurnTokens::default(),
    };
    let input = tokens.get("input").and_then(|x| x.as_i64());
    let output = tokens.get("output").and_then(|x| x.as_i64());
    let cache = tokens.get("cache");
    let cache_read = cache.and_then(|c| c.get("read")).and_then(|x| x.as_i64());
    let cache_creation = cache.and_then(|c| c.get("write")).and_then(|x| x.as_i64());
    TurnTokens {
        input: input.filter(|n| *n > 0),
        cache_read: cache_read.filter(|n| *n > 0),
        cache_creation: cache_creation.filter(|n| *n > 0),
        output: output.filter(|n| *n > 0),
    }
}

fn load_tool_names_per_message(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> std::collections::HashMap<String, Vec<String>> {
    let mut out: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let mut stmt = match conn.prepare(
        "SELECT message_id, data FROM part \
         WHERE session_id = ?1 ORDER BY time_created ASC",
    ) {
        Ok(s) => s,
        Err(_) => return out,
    };
    let rows = match stmt.query_map([session_id], |row| {
        let mid: String = row.get(0)?;
        let data: String = row.get(1)?;
        Ok((mid, data))
    }) {
        Ok(r) => r,
        Err(_) => return out,
    };
    for (mid, data_str) in rows.flatten() {
        let v: serde_json::Value = match serde_json::from_str(&data_str) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("tool") {
            continue;
        }
        // Part json shape: {"type": "tool", "tool": "read",
        //   "state": {"input": ..., "output": ...}}
        if let Some(name) = v.get("tool").and_then(|t| t.as_str()) {
            out.entry(mid).or_default().push(name.to_string());
        }
    }
    out
}

fn load_text_per_message(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> std::collections::HashMap<String, String> {
    let mut out: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let mut stmt = match conn.prepare(
        "SELECT message_id, data FROM part \
         WHERE session_id = ?1 ORDER BY time_created ASC",
    ) {
        Ok(s) => s,
        Err(_) => return std::collections::HashMap::new(),
    };
    let rows = match stmt.query_map([session_id], |row| {
        let mid: String = row.get(0)?;
        let data: String = row.get(1)?;
        Ok((mid, data))
    }) {
        Ok(r) => r,
        Err(_) => return std::collections::HashMap::new(),
    };
    for (mid, data_str) in rows.flatten() {
        let v: serde_json::Value = match serde_json::from_str(&data_str) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("text") {
            continue;
        }
        if let Some(t) = v.get("text").and_then(|s| s.as_str()) {
            out.entry(mid).or_default().push(t.to_string());
        }
    }
    out.into_iter().map(|(k, v)| (k, v.join("\n"))).collect()
}

fn ingest_legacy(
    conn: &rusqlite::Connection,
    session_id: &str,
    sink: &mut dyn TurnSink,
) -> Result<TapSummary, TapError> {
    // Legacy OpenCode DBs store only session-level aggregates. The
    // cleanest honest answer is: no per-turn deltas exist in the
    // source, so the tap emits a single rolled-up assistant turn
    // rather than spreading fake deltas across message rows.
    let mut summary = TapSummary::default();

    let row: Result<(i64, i64, f64, i64, i64), _> = conn.query_row(
        "SELECT prompt_tokens, completion_tokens, cost, created_at, updated_at \
         FROM session WHERE id = ?1",
        [session_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    );

    let (prompt, completion, cost, created_at, updated_at) = match row {
        Ok(t) => t,
        Err(_) => {
            summary
                .warnings
                .push(format!("opencode: legacy session {session_id} not found"));
            return Ok(summary);
        }
    };

    if prompt == 0 && completion == 0 {
        summary.warnings.push(format!(
            "opencode: legacy session {session_id} has no token data"
        ));
        return Ok(summary);
    }

    let tokens = TurnTokens {
        input: (prompt > 0).then_some(prompt),
        cache_read: None,
        cache_creation: None,
        output: (completion > 0).then_some(completion),
    };

    let started = Some(normalize_ts(created_at));
    let ended = Some(normalize_ts(updated_at));

    let turn = Turn {
        id: Turn::deterministic_id(SOURCE, session_id, 0),
        session_id: session_id.to_string(),
        source: SOURCE.to_string(),
        turn_index: 0,
        role: TurnRole::Assistant,
        started_at: started,
        ended_at: ended,
        model: None,
        tokens,
        cost_usd: (cost > 0.0).then_some(cost),
        tool_call_count: 0,
        thinking_ms: None,
        message_preview: Some("(legacy OpenCode session — aggregated turn)".into()),
        raw_ref: Some(format!("opencode-legacy:{session_id}")),
        tool_names: None,
    };

    sink.accept_turn(turn);
    summary.turns_emitted = 1;
    summary.warnings.push(
        "opencode: legacy schema only exposes session-level aggregates; emitted one synthetic turn"
            .into(),
    );
    Ok(summary)
}

fn normalize_ts(ts: i64) -> i64 {
    if ts > 1_000_000_000_000 {
        ts
    } else {
        ts * 1000
    }
}

fn clip_preview(s: &str) -> String {
    let one_line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = one_line.chars().collect();
    if chars.len() <= 160 {
        chars.into_iter().collect()
    } else {
        let mut out: String = chars.into_iter().take(157).collect();
        out.push_str("...");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taps::memory_sink::MemorySink;
    use rusqlite::Connection;

    fn modern_schema(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE project (id TEXT PRIMARY KEY, worktree TEXT, time_created INT, time_updated INT);
             CREATE TABLE session (id TEXT PRIMARY KEY, project_id TEXT, title TEXT, time_created INT, time_updated INT);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INT, time_updated INT, data TEXT);
             CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INT, time_updated INT, data TEXT);",
        )
        .unwrap();
    }

    fn legacy_schema(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, title TEXT, message_count INT, prompt_tokens INT, completion_tokens INT, cost REAL, created_at INT, updated_at INT);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, role TEXT, content TEXT, created_at INT);",
        )
        .unwrap();
    }

    #[test]
    fn modern_emits_one_turn_per_message_with_per_call_tokens() {
        let tmp = tempfile::TempDir::new().unwrap();
        let p = tmp.path().join("opencode.db");
        let conn = Connection::open(&p).unwrap();
        modern_schema(&conn);
        conn.execute("INSERT INTO project VALUES ('p', '/tmp', 0, 0)", [])
            .unwrap();
        conn.execute("INSERT INTO session VALUES ('s', 'p', 't', 1000, 2000)", [])
            .unwrap();
        conn.execute(
            r#"INSERT INTO message VALUES ('m1', 's', 1000, 1000, '{"role":"user"}')"#,
            [],
        )
        .unwrap();
        conn.execute(
            r#"INSERT INTO message VALUES ('m2', 's', 1100, 1100,
                '{"role":"assistant","modelID":"claude-sonnet","cost":0.01,
                  "tokens":{"input":50,"output":20,"cache":{"read":200,"write":10}},
                  "finish":"stop"}')"#,
            [],
        )
        .unwrap();
        conn.execute(
            r#"INSERT INTO part VALUES ('p1', 'm1', 's', 1000, 1000, '{"type":"text","text":"hello"}')"#,
            [],
        )
        .unwrap();
        conn.execute(
            r#"INSERT INTO part VALUES ('p2', 'm2', 's', 1100, 1100, '{"type":"text","text":"hi!"}')"#,
            [],
        )
        .unwrap();
        conn.execute(
            r#"INSERT INTO part VALUES ('p3', 'm2', 's', 1101, 1101, '{"type":"tool","tool":"read","state":{"input":{}}}')"#,
            [],
        )
        .unwrap();
        drop(conn);

        let mut sink = MemorySink::default();
        let summary = OpenCodeTurnTap
            .ingest_session("s", TapArtifact::File(&p), &mut sink)
            .unwrap();
        assert_eq!(summary.turns_emitted, 2);

        assert_eq!(sink.turns[0].role, TurnRole::User);
        assert_eq!(sink.turns[0].turn_index, 0);
        assert_eq!(sink.turns[0].message_preview.as_deref(), Some("hello"));

        assert_eq!(sink.turns[1].role, TurnRole::Assistant);
        assert_eq!(sink.turns[1].turn_index, 1);
        assert_eq!(sink.turns[1].tokens.input, Some(50));
        assert_eq!(sink.turns[1].tokens.output, Some(20));
        assert_eq!(sink.turns[1].tokens.cache_read, Some(200));
        assert_eq!(sink.turns[1].tokens.cache_creation, Some(10));
        assert_eq!(sink.turns[1].model.as_deref(), Some("claude-sonnet"));
        assert_eq!(sink.turns[1].cost_usd, Some(0.01));
        assert_eq!(sink.turns[1].tool_call_count, 1);
        assert_eq!(sink.turns[1].tool_names.as_deref(), Some("read"));
    }

    #[test]
    fn legacy_emits_single_synthetic_turn() {
        let tmp = tempfile::TempDir::new().unwrap();
        let p = tmp.path().join("opencode.db");
        let conn = Connection::open(&p).unwrap();
        legacy_schema(&conn);
        conn.execute(
            "INSERT INTO session VALUES ('s', 't', 10, 1000, 500, 0.05, 1700000000, 1700000060)",
            [],
        )
        .unwrap();
        drop(conn);

        let mut sink = MemorySink::default();
        let summary = OpenCodeTurnTap
            .ingest_session("s", TapArtifact::File(&p), &mut sink)
            .unwrap();
        assert_eq!(summary.turns_emitted, 1);
        assert_eq!(sink.turns[0].tokens.input, Some(1000));
        assert_eq!(sink.turns[0].tokens.output, Some(500));
        assert_eq!(sink.turns[0].cost_usd, Some(0.05));
        assert!(!summary.warnings.is_empty());
    }

    #[test]
    fn legacy_missing_session_warns_not_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let p = tmp.path().join("opencode.db");
        let conn = Connection::open(&p).unwrap();
        legacy_schema(&conn);
        drop(conn);

        let mut sink = MemorySink::default();
        let summary = OpenCodeTurnTap
            .ingest_session("nope", TapArtifact::File(&p), &mut sink)
            .unwrap();
        assert_eq!(summary.turns_emitted, 0);
        assert!(!summary.warnings.is_empty());
    }
}
