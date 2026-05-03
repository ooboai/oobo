//! Codex (`codex-cli`) L1 tap.
//!
//! Codex stores rollouts as newline-delimited JSONL under
//! `~/.codex/sessions/YYYY/MM/DD/rollout-<session-id>.jsonl`.
//!
//! Event shape (relevant subset):
//! ```json
//! {"timestamp": "...", "type": "session_meta", "payload": {"model": "gpt-5.3-codex", "cwd": "..."}}
//! {"timestamp": "...", "type": "event_msg", "payload": {"type": "user_message", "message": "..."}}
//! {"timestamp": "...", "type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [...]}}
//! {"timestamp": "...", "type": "response_item", "payload": {"type": "function_call", "name": "apply_patch", "arguments": "..."}}
//! {"timestamp": "...", "type": "event_msg", "payload": {"type": "token_count",
//!   "info": {"total_token_usage": {"input_tokens": 1000, "output_tokens": 500, "cached_input_tokens": 200}}}}
//! ```
//!
//! **Important:** Codex reports `total_token_usage` as a **cumulative**
//! running total, not a per-call delta. To hand the new model honest
//! per-turn deltas, the tap subtracts the previous `token_count`
//! values from each new one and attributes the difference to the
//! most recent assistant response — which is exactly the call that
//! was billed.
//!
//! Turn boundaries:
//! - A `user_message` event is one user turn.
//! - A run of `response_item`s (message + function_call + tool_call +
//!   reasoning) between two `token_count` events is **one assistant
//!   turn**, attributed the delta of that `token_count`.
//!
//! Codex has no native subagent concept; the tap emits no
//! [`SubagentLink`]s.

use std::fs;
use std::io::BufRead;
use std::path::Path;

use serde_json::Value;

use super::{Source, TapArtifact, TapError, TapSummary, TurnSink, TurnTap};
use crate::config::Config;
use crate::core::turn::{Turn, TurnRole, TurnTokens};

pub const SOURCE: Source = "codex";

pub struct CodexTurnTap;

impl TurnTap for CodexTurnTap {
    fn source(&self) -> Source {
        SOURCE
    }

    fn enabled(&self, cfg: &Config) -> bool {
        cfg.codex.enabled
    }

    fn ingest_session(
        &self,
        session_id: &str,
        artifact: TapArtifact<'_>,
        sink: &mut dyn TurnSink,
    ) -> Result<TapSummary, TapError> {
        match artifact {
            TapArtifact::File(path) => ingest_file(session_id, path, sink),
            _ => Err(TapError::Other(
                "codex tap only supports TapArtifact::File".into(),
            )),
        }
    }
}

/// Accumulator for an in-flight assistant turn. Flushed when a
/// `token_count` event arrives (the delta is the billing for this
/// run) or when a new user message interrupts it.
#[derive(Default)]
struct AssistantAccum {
    first_ts_ms: Option<i64>,
    last_ts_ms: Option<i64>,
    preview: Option<String>,
    tool_names: Vec<String>,
    tool_call_count: i64,
    /// `true` if at least one `response_item` event fired since
    /// the last flush. Guards us from flushing empty turns when the
    /// rollout starts with a `token_count` event.
    has_content: bool,
}

struct CumulativeTokens {
    input: i64,
    output: i64,
    cache_read: i64,
}

impl CumulativeTokens {
    fn zero() -> Self {
        Self {
            input: 0,
            output: 0,
            cache_read: 0,
        }
    }
}

fn ingest_file(
    session_id: &str,
    path: &Path,
    sink: &mut dyn TurnSink,
) -> Result<TapSummary, TapError> {
    let mut summary = TapSummary::default();
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(TapError::SourceMissing(path.display().to_string()))
        }
        Err(e) => return Err(TapError::Io(e)),
    };
    let reader = std::io::BufReader::new(file);

    let mut turn_index: i64 = 0;
    let mut model: Option<String> = None;
    let mut accum = AssistantAccum::default();
    let mut prev_cum = CumulativeTokens::zero();

    for (lineno, line_res) in reader.lines().enumerate() {
        let line = if let Ok(l) = line_res { l } else {
            summary.turns_skipped += 1;
            continue;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                summary
                    .warnings
                    .push(format!("codex: parse error at line {}: {}", lineno + 1, e));
                summary.turns_skipped += 1;
                continue;
            }
        };

        let ts_ms = parse_ts_ms(&v);
        let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let payload = v.get("payload");

        match event_type {
            "session_meta" => {
                if model.is_none() {
                    model = payload
                        .and_then(|p| p.get("model"))
                        .and_then(|m| m.as_str())
                        .map(std::string::ToString::to_string);
                }
            }
            "event_msg" => {
                let msg_type = payload
                    .and_then(|p| p.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                match msg_type {
                    "user_message" => {
                        // Interrupt any in-flight assistant accum
                        // as a zero-token turn so indices still line
                        // up with reality; the next token_count
                        // delta will land on the *next* assistant
                        // turn, which is correct.
                        flush_assistant(
                            session_id,
                            &mut turn_index,
                            &mut accum,
                            None,
                            model.as_ref(),
                            sink,
                            &mut summary,
                        );

                        let preview = payload
                            .and_then(|p| p.get("message"))
                            .and_then(|m| m.as_str())
                            .map(clip_preview);
                        let turn = Turn {
                            id: Turn::deterministic_id(SOURCE, session_id, turn_index),
                            session_id: session_id.to_string(),
                            source: SOURCE.to_string(),
                            turn_index,
                            role: TurnRole::User,
                            started_at: ts_ms,
                            ended_at: ts_ms,
                            model: model.clone(),
                            tokens: TurnTokens::default(),
                            cost_usd: None,
                            tool_call_count: 0,
                            thinking_ms: None,
                            message_preview: preview,
                            raw_ref: Some(format!("codex:{session_id}#{turn_index}")),
                            tool_names: None,
                        };
                        sink.accept_turn(turn);
                        summary.turns_emitted += 1;
                        turn_index += 1;
                    }
                    "token_count" => {
                        let cum = extract_cumulative(payload);
                        let delta = TurnTokens {
                            input: positive(cum.input - prev_cum.input),
                            cache_read: positive(cum.cache_read - prev_cum.cache_read),
                            cache_creation: None,
                            output: positive(cum.output - prev_cum.output),
                        };
                        prev_cum = cum;

                        flush_assistant(
                            session_id,
                            &mut turn_index,
                            &mut accum,
                            Some(delta),
                            model.as_ref(),
                            sink,
                            &mut summary,
                        );
                    }
                    _ => {}
                }
            }
            "response_item" => {
                accum.has_content = true;
                if accum.first_ts_ms.is_none() {
                    accum.first_ts_ms = ts_ms;
                }
                accum.last_ts_ms = ts_ms.or(accum.last_ts_ms);

                let item_type = payload
                    .and_then(|p| p.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                match item_type {
                    "function_call" | "tool_call" => {
                        accum.tool_call_count += 1;
                        if let Some(name) =
                            payload.and_then(|p| p.get("name")).and_then(|n| n.as_str())
                        {
                            accum.tool_names.push(name.to_string());
                        }
                    }
                    "message" => {
                        let role = payload
                            .and_then(|p| p.get("role"))
                            .and_then(|r| r.as_str())
                            .unwrap_or("assistant");
                        if role == "assistant" && accum.preview.is_none() {
                            accum.preview =
                                extract_message_text(payload).as_deref().map(clip_preview);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    // End of file: if an assistant turn is still in-flight without a
    // final token_count, flush it with unknown tokens. Better to
    // record the activity than drop it silently.
    if accum.has_content {
        flush_assistant(
            session_id,
            &mut turn_index,
            &mut accum,
            None,
            model.as_ref(),
            sink,
            &mut summary,
        );
        summary.warnings.push(
            "codex: trailing assistant activity with no closing token_count — tokens unknown"
                .into(),
        );
    }

    Ok(summary)
}

fn flush_assistant(
    session_id: &str,
    turn_index: &mut i64,
    accum: &mut AssistantAccum,
    tokens: Option<TurnTokens>,
    model: Option<&String>,
    sink: &mut dyn TurnSink,
    summary: &mut TapSummary,
) {
    if !accum.has_content {
        // A token_count with no preceding assistant activity is
        // ignorable — but we still consumed the delta by updating
        // prev_cum. No turn emitted, no warning (this happens
        // naturally at session start).
        *accum = AssistantAccum::default();
        return;
    }

    let tool_names = if accum.tool_names.is_empty() {
        None
    } else {
        Some(accum.tool_names.join(","))
    };

    let turn = Turn {
        id: Turn::deterministic_id(SOURCE, session_id, *turn_index),
        session_id: session_id.to_string(),
        source: SOURCE.to_string(),
        turn_index: *turn_index,
        role: TurnRole::Assistant,
        started_at: accum.first_ts_ms,
        ended_at: accum.last_ts_ms.or(accum.first_ts_ms),
        model: model.cloned(),
        tokens: tokens.unwrap_or_default(),
        cost_usd: None,
        tool_call_count: accum.tool_call_count,
        thinking_ms: None,
        message_preview: accum.preview.take(),
        raw_ref: Some(format!("codex:{session_id}#{}", *turn_index)),
        tool_names,
    };

    sink.accept_turn(turn);
    summary.turns_emitted += 1;
    *turn_index += 1;
    *accum = AssistantAccum::default();
}

fn extract_cumulative(payload: Option<&Value>) -> CumulativeTokens {
    let info = match payload.and_then(|p| p.get("info")) {
        Some(i) => i,
        None => return CumulativeTokens::zero(),
    };
    let total = match info.get("total_token_usage") {
        Some(t) => t,
        None => return CumulativeTokens::zero(),
    };
    CumulativeTokens {
        input: total
            .get("input_tokens")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
        output: total
            .get("output_tokens")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
        cache_read: total
            .get("cached_input_tokens")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
    }
}

fn extract_message_text(payload: Option<&Value>) -> Option<String> {
    let content = payload?.get("content")?.as_array()?;
    let parts: Vec<String> = content
        .iter()
        .filter_map(|item| {
            item.get("text")
                .and_then(|t| t.as_str())
                .map(std::string::ToString::to_string)
        })
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn parse_ts_ms(v: &Value) -> Option<i64> {
    v.get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(crate::utils::parse_iso_timestamp)
        .or_else(|| v.get("timestamp").and_then(serde_json::Value::as_i64))
}

fn positive(n: i64) -> Option<i64> {
    if n > 0 {
        Some(n)
    } else {
        None
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
    use std::io::Write;

    fn write_tmp(lines: &[serde_json::Value]) -> tempfile::NamedTempFile {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut f = tmp.reopen().unwrap();
        for v in lines {
            writeln!(f, "{}", serde_json::to_string(v).unwrap()).unwrap();
        }
        tmp
    }

    #[test]
    fn emits_per_turn_deltas_from_cumulative_token_counts() {
        let lines = vec![
            serde_json::json!({"timestamp": "2026-04-22T10:00:00Z", "type": "session_meta",
                "payload": {"model": "gpt-5.3-codex", "cwd": "/tmp/proj"}}),
            serde_json::json!({"timestamp": "2026-04-22T10:00:01Z", "type": "event_msg",
                "payload": {"type": "user_message", "message": "hello"}}),
            serde_json::json!({"timestamp": "2026-04-22T10:00:02Z", "type": "response_item",
                "payload": {"type": "message", "role": "assistant",
                    "content": [{"type":"text","text":"hi there"}]}}),
            serde_json::json!({"timestamp": "2026-04-22T10:00:03Z", "type": "event_msg",
                "payload": {"type": "token_count",
                    "info": {"total_token_usage": {"input_tokens": 100, "output_tokens": 20, "cached_input_tokens": 0}}}}),
            serde_json::json!({"timestamp": "2026-04-22T10:00:04Z", "type": "response_item",
                "payload": {"type": "function_call", "name": "apply_patch", "arguments": "{}"}}),
            serde_json::json!({"timestamp": "2026-04-22T10:00:05Z", "type": "response_item",
                "payload": {"type": "message", "role": "assistant",
                    "content": [{"type":"text","text":"patched"}]}}),
            serde_json::json!({"timestamp": "2026-04-22T10:00:06Z", "type": "event_msg",
                "payload": {"type": "token_count",
                    "info": {"total_token_usage": {"input_tokens": 250, "output_tokens": 60, "cached_input_tokens": 100}}}}),
        ];
        let tmp = write_tmp(&lines);
        let mut sink = MemorySink::default();
        let summary = CodexTurnTap
            .ingest_session("sess-1", TapArtifact::File(tmp.path()), &mut sink)
            .unwrap();
        assert_eq!(summary.turns_emitted, 3);
        assert_eq!(sink.turns.len(), 3);

        // turn 0: user
        assert_eq!(sink.turns[0].role, TurnRole::User);
        // turn 1: assistant, delta input=100, output=20
        assert_eq!(sink.turns[1].role, TurnRole::Assistant);
        assert_eq!(sink.turns[1].tokens.input, Some(100));
        assert_eq!(sink.turns[1].tokens.output, Some(20));
        assert_eq!(sink.turns[1].tokens.cache_read, None);
        // turn 2: assistant, delta input=150, output=40, cache=100
        assert_eq!(sink.turns[2].role, TurnRole::Assistant);
        assert_eq!(sink.turns[2].tokens.input, Some(150));
        assert_eq!(sink.turns[2].tokens.output, Some(40));
        assert_eq!(sink.turns[2].tokens.cache_read, Some(100));
        assert_eq!(sink.turns[2].tool_call_count, 1);
        assert_eq!(sink.turns[2].tool_names.as_deref(), Some("apply_patch"));
        assert_eq!(sink.turns[2].model.as_deref(), Some("gpt-5.3-codex"));
    }

    #[test]
    fn unknown_event_types_do_not_create_turns() {
        let lines = vec![
            serde_json::json!({"timestamp": "2026-04-22T10:00:00Z", "type": "unknown_thing"}),
            serde_json::json!({"timestamp": "2026-04-22T10:00:01Z", "type": "event_msg",
                "payload": {"type": "user_message", "message": "hi"}}),
        ];
        let tmp = write_tmp(&lines);
        let mut sink = MemorySink::default();
        let summary = CodexTurnTap
            .ingest_session("sess-2", TapArtifact::File(tmp.path()), &mut sink)
            .unwrap();
        assert_eq!(summary.turns_emitted, 1);
        assert_eq!(sink.turns[0].role, TurnRole::User);
    }

    #[test]
    fn token_count_before_any_assistant_activity_is_absorbed() {
        let lines = vec![
            serde_json::json!({"timestamp": "2026-04-22T10:00:00Z", "type": "event_msg",
                "payload": {"type": "token_count",
                    "info": {"total_token_usage": {"input_tokens": 5, "output_tokens": 0, "cached_input_tokens": 0}}}}),
            serde_json::json!({"timestamp": "2026-04-22T10:00:01Z", "type": "response_item",
                "payload": {"type": "message", "role": "assistant",
                    "content": [{"type":"text","text":"ok"}]}}),
            serde_json::json!({"timestamp": "2026-04-22T10:00:02Z", "type": "event_msg",
                "payload": {"type": "token_count",
                    "info": {"total_token_usage": {"input_tokens": 15, "output_tokens": 7, "cached_input_tokens": 0}}}}),
        ];
        let tmp = write_tmp(&lines);
        let mut sink = MemorySink::default();
        let _ = CodexTurnTap
            .ingest_session("s", TapArtifact::File(tmp.path()), &mut sink)
            .unwrap();
        assert_eq!(sink.turns.len(), 1);
        // delta = 15-5, 7-0 = 10, 7
        assert_eq!(sink.turns[0].tokens.input, Some(10));
        assert_eq!(sink.turns[0].tokens.output, Some(7));
    }

    #[test]
    fn rejects_wrong_artifact_type() {
        let mut sink = MemorySink::default();
        let err = CodexTurnTap.ingest_session("s", TapArtifact::SelfLookup, &mut sink);
        assert!(err.is_err());
    }

    #[test]
    fn reingestion_produces_identical_ids() {
        let lines = vec![
            serde_json::json!({"timestamp": "2026-04-22T10:00:00Z", "type": "event_msg",
                "payload": {"type": "user_message", "message": "hi"}}),
            serde_json::json!({"timestamp": "2026-04-22T10:00:01Z", "type": "response_item",
                "payload": {"type": "message", "role": "assistant",
                    "content": [{"type":"text","text":"ok"}]}}),
            serde_json::json!({"timestamp": "2026-04-22T10:00:02Z", "type": "event_msg",
                "payload": {"type": "token_count",
                    "info": {"total_token_usage": {"input_tokens": 10, "output_tokens": 5, "cached_input_tokens": 0}}}}),
        ];
        let tmp = write_tmp(&lines);
        let mut a = MemorySink::default();
        let mut b = MemorySink::default();
        CodexTurnTap
            .ingest_session("s", TapArtifact::File(tmp.path()), &mut a)
            .unwrap();
        CodexTurnTap
            .ingest_session("s", TapArtifact::File(tmp.path()), &mut b)
            .unwrap();
        let a_ids: Vec<_> = a.turns.iter().map(|t| &t.id).collect();
        let b_ids: Vec<_> = b.turns.iter().map(|t| &t.id).collect();
        assert_eq!(a_ids, b_ids);
    }
}
