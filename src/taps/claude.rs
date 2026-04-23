//! Claude Code L1 tap.
//!
//! Claude's native artifact is a newline-delimited JSONL transcript.
//! Each `{"type": "assistant", ...}` entry carries a `message.usage`
//! block with the exact per-call token deltas the API billed:
//!
//! ```json
//! {
//!   "type": "assistant",
//!   "uuid": "m-123",
//!   "timestamp": "2026-04-22T10:00:00Z",
//!   "message": {
//!     "model": "claude-opus-4-5",
//!     "usage": {
//!       "input_tokens": 10,
//!       "output_tokens": 42,
//!       "cache_read_input_tokens": 1000,
//!       "cache_creation_input_tokens": 50
//!     },
//!     "content": [{"type": "text", "text": "..."},
//!                 {"type": "tool_use", "name": "Task", ...}]
//!   }
//! }
//! ```
//!
//! These are **per-call deltas**, exactly what the new model demands.
//! The old session-level stats collector had been summing them already
//! into `session_stats` — what it was missing was keeping the per-turn
//! granularity. This tap fixes that.
//!
//! Subagents: Claude writes subagent transcripts under
//! `~/.claude/projects/<slug>/<session>/subagents/<sub_id>.jsonl`.
//! When the caller provides them via
//! [`TapArtifact::FileWithSubagents`], the tap processes each as its
//! own session and emits a [`SubagentLink`] that the store will
//! persist onto `sessions.parent_session_id`.

use std::fs;
use std::io::BufRead;
use std::path::Path;

use serde_json::Value;

use super::{SubagentLink, Source, TapArtifact, TapError, TapSummary, TurnSink, TurnTap};
use crate::config::Config;
use crate::core::turn::{Turn, TurnRole, TurnTokens};

pub const SOURCE: Source = "claude";

pub struct ClaudeTurnTap;

impl TurnTap for ClaudeTurnTap {
    fn source(&self) -> Source {
        SOURCE
    }

    fn enabled(&self, cfg: &Config) -> bool {
        cfg.claude.enabled
    }

    fn ingest_session(
        &self,
        session_id: &str,
        artifact: TapArtifact<'_>,
        sink: &mut dyn TurnSink,
    ) -> Result<TapSummary, TapError> {
        match artifact {
            TapArtifact::File(path) => ingest_one_file(SOURCE, session_id, path, sink),
            TapArtifact::FileWithSubagents { primary, subagents } => {
                let mut total = ingest_one_file(SOURCE, session_id, primary, sink)?;
                for (sub_id, sub_path) in subagents {
                    let sub_summary = ingest_one_file(SOURCE, sub_id, sub_path, sink)?;
                    total = total.merged(sub_summary);
                    sink.accept_subagent_link(SubagentLink {
                        child_session_id: sub_id.clone(),
                        child_source: SOURCE.to_string(),
                        parent_session_id: session_id.to_string(),
                        parent_source: SOURCE.to_string(),
                        // Turn-precision parent link is filled in by M4's
                        // inference pass (matching Task tool_use ids).
                        parent_turn_id: None,
                        subagent_kind: Some("task".into()),
                    });
                    total.subagent_links_emitted += 1;
                }
                Ok(total)
            }
        }
    }
}

/// Process one JSONL file as a flat turn stream.
fn ingest_one_file(
    source: Source,
    session_id: &str,
    path: &Path,
    sink: &mut dyn TurnSink,
) -> Result<TapSummary, TapError> {
    let file = fs::File::open(path).map_err(TapError::Io)?;
    let reader = std::io::BufReader::new(file);

    let mut summary = TapSummary::default();
    // Monotonic turn index, 0-based, stable across re-ingest because
    // the transcript file is append-only.
    let mut turn_index: i64 = 0;

    let now_ms = chrono::Utc::now().timestamp_millis();

    for (lineno, line) in reader.lines().enumerate().filter_map(|(i, r)| r.ok().map(|l| (i, l))) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(err) => {
                summary.turns_skipped += 1;
                summary.warnings.push(format!(
                    "{}:{}: malformed json: {}",
                    path.display(),
                    lineno + 1,
                    err
                ));
                continue;
            }
        };

        let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let role = match entry_type {
            "user" => TurnRole::User,
            "assistant" => TurnRole::Assistant,
            "system" => TurnRole::System,
            "tool_result" => TurnRole::Tool,
            // result/meta/etc aren't conversational turns.
            _ => {
                summary.turns_skipped += 1;
                continue;
            }
        };

        let ts_ms = parse_timestamp(&entry);
        let (tokens, model, thinking_ms, tool_call_count, tool_names) =
            extract_assistant_metadata(&entry);
        let preview = extract_preview(&entry, role);
        let raw_ref = format!("jsonl:{}#{}", path.display(), lineno + 1);

        let turn = Turn {
            id: Turn::deterministic_id(source, session_id, turn_index),
            session_id: session_id.to_string(),
            source: source.to_string(),
            turn_index,
            role,
            started_at: ts_ms,
            ended_at: ts_ms,
            model,
            tokens,
            // Cost derivation happens at the store layer where the
            // model→pricing table lives; taps stay dumb about rates.
            cost_usd: None,
            tool_call_count,
            thinking_ms,
            message_preview: preview,
            raw_ref: Some(raw_ref),
            tool_names,
        };

        sink.accept_turn(turn);
        summary.turns_emitted += 1;
        turn_index += 1;
    }

    // Capture the ingestion epoch in a warning-less way by embedding
    // it in the summary if zero turns made it through — useful signal
    // for debugging empty transcripts.
    if summary.turns_emitted == 0 {
        summary
            .warnings
            .push(format!("no turns extracted from {}", path.display()));
    }
    let _ = now_ms; // reserved for future per-run timing metrics

    Ok(summary)
}

/// Pull model, per-call usage, thinking duration, and tool-call count
/// out of an assistant entry. Returns defaults for non-assistant
/// entries (non-assistant turns have no tokens).
fn extract_assistant_metadata(
    entry: &Value,
) -> (TurnTokens, Option<String>, Option<i64>, i64, Option<String>) {
    let msg = match entry.get("message") {
        Some(m) => m,
        None => return (TurnTokens::default(), None, None, 0, None),
    };

    let model = msg
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let tokens = match msg.get("usage") {
        Some(u) => TurnTokens {
            input: u.get("input_tokens").and_then(|v| v.as_i64()),
            cache_read: u.get("cache_read_input_tokens").and_then(|v| v.as_i64()),
            cache_creation: u
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_i64()),
            output: u.get("output_tokens").and_then(|v| v.as_i64()),
        },
        None => TurnTokens::default(),
    };

    let mut thinking_ms: Option<i64> = None;
    let mut tool_call_count: i64 = 0;
    let mut tool_names: Vec<String> = Vec::new();

    if let Some(content) = msg.get("content").and_then(|v| v.as_array()) {
        for part in content {
            match part.get("type").and_then(|v| v.as_str()) {
                Some("tool_use") => {
                    tool_call_count += 1;
                    if let Some(name) = part.get("name").and_then(|v| v.as_str()) {
                        tool_names.push(name.to_string());
                    }
                }
                Some("thinking") => {
                    // Claude currently reports thinking content but not
                    // ms; left as a reserved slot until the tool
                    // exposes the duration field.
                    if let Some(ms) = part.get("thinking_ms").and_then(|v| v.as_i64()) {
                        thinking_ms = Some(thinking_ms.unwrap_or(0) + ms);
                    }
                }
                _ => {}
            }
        }
    }

    let tool_names_joined = if tool_names.is_empty() {
        None
    } else {
        Some(tool_names.join(","))
    };

    (tokens, model, thinking_ms, tool_call_count, tool_names_joined)
}

fn parse_timestamp(entry: &Value) -> Option<i64> {
    if let Some(ts_str) = entry.get("timestamp").and_then(|v| v.as_str()) {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts_str) {
            return Some(dt.timestamp_millis());
        }
    }
    entry.get("timestamp").and_then(|v| v.as_i64())
}

/// Extract a short redacted preview suitable for the TUI / search.
/// Caps at 160 chars to keep DB rows small; full content is reachable
/// via `raw_ref`.
fn extract_preview(entry: &Value, role: TurnRole) -> Option<String> {
    let msg = entry.get("message")?;
    let raw = match role {
        TurnRole::User => {
            // User content can be a bare string or a content array.
            match msg.get("content") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(arr)) => arr
                    .iter()
                    .filter_map(|p| {
                        p.get("text")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => return None,
            }
        }
        TurnRole::Assistant => {
            let arr = msg.get("content")?.as_array()?;
            arr.iter()
                .filter_map(|p| match p.get("type").and_then(|v| v.as_str()) {
                    Some("text") => p.get("text").and_then(|v| v.as_str()).map(String::from),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
        _ => return None,
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Single-line the preview and cap.
    let one_line: String = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = one_line.chars().collect();
    if chars.len() <= 160 {
        Some(chars.into_iter().collect())
    } else {
        let mut preview: String = chars.into_iter().take(157).collect();
        preview.push_str("...");
        Some(preview)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taps::memory_sink::MemorySink;
    use std::io::Write;

    fn fixture(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        f.flush().unwrap();
        f
    }

    #[test]
    fn emits_one_turn_per_entry_with_native_deltas() {
        let f = fixture(&[
            r#"{"type":"user","message":{"content":"Hello"},"uuid":"u1","timestamp":"2026-04-22T10:00:00Z"}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-4-5","usage":{"input_tokens":5,"output_tokens":40,"cache_read_input_tokens":1000,"cache_creation_input_tokens":10},"content":[{"type":"text","text":"Hi there"}]},"uuid":"a1","timestamp":"2026-04-22T10:00:01Z"}"#,
        ]);
        let mut sink = MemorySink::default();
        let summary = ClaudeTurnTap
            .ingest_session("sess-1", TapArtifact::File(f.path()), &mut sink)
            .unwrap();
        assert_eq!(summary.turns_emitted, 2);
        assert_eq!(sink.turns.len(), 2);

        let user = &sink.turns[0];
        assert_eq!(user.role, TurnRole::User);
        assert_eq!(user.turn_index, 0);
        assert_eq!(user.message_preview.as_deref(), Some("Hello"));
        assert!(!user.tokens.has_any(), "user turn has no tokens");

        let assistant = &sink.turns[1];
        assert_eq!(assistant.role, TurnRole::Assistant);
        assert_eq!(assistant.turn_index, 1);
        assert_eq!(assistant.model.as_deref(), Some("claude-opus-4-5"));
        assert_eq!(assistant.tokens.input, Some(5));
        assert_eq!(assistant.tokens.output, Some(40));
        assert_eq!(assistant.tokens.cache_read, Some(1000));
        assert_eq!(assistant.tokens.cache_creation, Some(10));
        // Exactly what the API billed for THIS call. Not cumulative.
        assert_eq!(assistant.tokens.billed(), 1055);
        assert_eq!(assistant.message_preview.as_deref(), Some("Hi there"));
    }

    #[test]
    fn turn_indices_are_contiguous_zero_based_and_stable() {
        let f = fixture(&[
            r#"{"type":"user","message":{"content":"a"}}"#,
            r#"{"type":"assistant","message":{"usage":{"output_tokens":1},"content":[{"type":"text","text":"b"}]}}"#,
            r#"{"type":"user","message":{"content":"c"}}"#,
        ]);
        let mut sink = MemorySink::default();
        ClaudeTurnTap
            .ingest_session("s", TapArtifact::File(f.path()), &mut sink)
            .unwrap();
        let indices: Vec<i64> = sink.turns.iter().map(|t| t.turn_index).collect();
        assert_eq!(indices, vec![0, 1, 2]);

        // Same file → same ids on re-ingest (idempotency key).
        let mut sink2 = MemorySink::default();
        ClaudeTurnTap
            .ingest_session("s", TapArtifact::File(f.path()), &mut sink2)
            .unwrap();
        for (a, b) in sink.turns.iter().zip(sink2.turns.iter()) {
            assert_eq!(a.id, b.id);
        }
    }

    #[test]
    fn malformed_lines_become_warnings_not_errors() {
        let f = fixture(&[
            r#"{"type":"user","message":{"content":"ok"}}"#,
            r#"{ not json"#,
            r#"{"type":"assistant","message":{"usage":{"output_tokens":2},"content":[]}}"#,
        ]);
        let mut sink = MemorySink::default();
        let summary = ClaudeTurnTap
            .ingest_session("s", TapArtifact::File(f.path()), &mut sink)
            .unwrap();
        assert_eq!(summary.turns_emitted, 2);
        assert_eq!(summary.turns_skipped, 1);
        assert_eq!(summary.warnings.len(), 1);
        assert!(summary.warnings[0].contains("malformed json"));
    }

    #[test]
    fn tool_calls_count_rolls_up_per_turn() {
        let f = fixture(&[
            r#"{"type":"assistant","message":{"usage":{"output_tokens":5},"content":[{"type":"text","text":"I'll use tools"},{"type":"tool_use","name":"Read","id":"tu_1","input":{}},{"type":"tool_use","name":"Write","id":"tu_2","input":{}}]}}"#,
        ]);
        let mut sink = MemorySink::default();
        ClaudeTurnTap
            .ingest_session("s", TapArtifact::File(f.path()), &mut sink)
            .unwrap();
        assert_eq!(sink.turns[0].tool_call_count, 2);
        assert_eq!(
            sink.turns[0].tool_names.as_deref(),
            Some("Read,Write"),
            "tool names captured in invocation order for M4 inference",
        );
    }

    #[test]
    fn task_tool_is_captured_in_tool_names_for_inference() {
        let f = fixture(&[
            r#"{"type":"assistant","message":{"usage":{"output_tokens":3},"content":[{"type":"tool_use","name":"Task","id":"tu_1","input":{"subagent_type":"explore","prompt":"Find X"}}]}}"#,
        ]);
        let mut sink = MemorySink::default();
        ClaudeTurnTap
            .ingest_session("s", TapArtifact::File(f.path()), &mut sink)
            .unwrap();
        let t = &sink.turns[0];
        assert_eq!(t.tool_names.as_deref(), Some("Task"));
        assert_eq!(t.tool_call_count, 1);
    }

    #[test]
    fn subagent_artifacts_emit_child_turns_and_link() {
        let parent = fixture(&[
            r#"{"type":"assistant","message":{"usage":{"output_tokens":3},"content":[{"type":"tool_use","name":"Task","id":"tu_x","input":{"subagent_type":"explore"}}]}}"#,
        ]);
        let child = fixture(&[
            r#"{"type":"user","message":{"content":"explore it"}}"#,
            r#"{"type":"assistant","message":{"usage":{"output_tokens":10},"content":[{"type":"text","text":"found things"}]}}"#,
        ]);
        let subs = vec![("sub-1".to_string(), child.path().to_path_buf())];
        let mut sink = MemorySink::default();
        let summary = ClaudeTurnTap
            .ingest_session(
                "parent-1",
                TapArtifact::FileWithSubagents {
                    primary: parent.path(),
                    subagents: &subs,
                },
                &mut sink,
            )
            .unwrap();
        assert_eq!(summary.turns_emitted, 3);
        assert_eq!(summary.subagent_links_emitted, 1);

        let parent_turns: Vec<_> = sink
            .turns
            .iter()
            .filter(|t| t.session_id == "parent-1")
            .collect();
        let child_turns: Vec<_> = sink
            .turns
            .iter()
            .filter(|t| t.session_id == "sub-1")
            .collect();
        assert_eq!(parent_turns.len(), 1);
        assert_eq!(child_turns.len(), 2);

        assert_eq!(sink.subagent_links.len(), 1);
        let link = &sink.subagent_links[0];
        assert_eq!(link.child_session_id, "sub-1");
        assert_eq!(link.parent_session_id, "parent-1");
        assert_eq!(link.subagent_kind.as_deref(), Some("task"));
    }

    #[test]
    fn preview_is_trimmed_collapsed_and_capped() {
        let long = "x".repeat(500);
        let f = fixture(&[&format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{long}"}}]}}}}"#
        )]);
        let mut sink = MemorySink::default();
        ClaudeTurnTap
            .ingest_session("s", TapArtifact::File(f.path()), &mut sink)
            .unwrap();
        let preview = sink.turns[0].message_preview.as_ref().unwrap();
        assert_eq!(preview.chars().count(), 160);
        assert!(preview.ends_with("..."));
    }

    #[test]
    fn missing_file_is_hard_error() {
        let mut sink = MemorySink::default();
        let err = ClaudeTurnTap.ingest_session(
            "s",
            TapArtifact::File(Path::new("/nonexistent/path.jsonl")),
            &mut sink,
        );
        assert!(err.is_err());
    }
}
