//! Cursor L1 tap.
//!
//! Cursor stores a composer session as a set of rows in the global
//! `state.vscdb` SQLite database, one row per message "bubble":
//!
//! - Key: `bubbleId:<session>:<uuid>`
//! - Value: JSON with `type` (1=user, 2=assistant), `text`,
//!   `tokenCount.inputTokens`, `tokenCount.outputTokens`,
//!   `createdAt`, `thinking.text`, `toolFormerData.{name,result}`.
//!
//! **`tokenCount` is per-bubble**  --  exactly the per-call delta the
//! new model wants. Unlike Claude's prompt-cache split, Cursor
//! records only `input_tokens` and `output_tokens`; cache metrics
//! stay `None`. Cost is `None` too (Cursor doesn't expose pricing
//! per-call; the downstream cost table can fill that in if/when we
//! know the model  --  which Cursor also doesn't persist per-session).
//!
//! Subagent hierarchy: Cursor has no native notion of subagents on
//! disk. The tap emits no [`SubagentLink`]; M4's inference engine
//! handles Cursor's nested-composer case via signals (future
//! `SubdirectoryArtifact` signal) rather than baking guesses in
//! here.

use super::{Source, TapArtifact, TapError, TapSummary, TurnSink, TurnTap};
use crate::config::Config;
use crate::core::turn::{Turn, TurnRole, TurnTokens};
use crate::tools::cursor::composer_data::{read_bubbles, CursorBubble};

pub const SOURCE: Source = "composer";

pub struct CursorTurnTap;

impl TurnTap for CursorTurnTap {
    fn source(&self) -> Source {
        SOURCE
    }

    fn enabled(&self, cfg: &Config) -> bool {
        cfg.cursor.enabled
    }

    fn ingest_session(
        &self,
        session_id: &str,
        artifact: TapArtifact<'_>,
        sink: &mut dyn TurnSink,
    ) -> Result<TapSummary, TapError> {
        match artifact {
            TapArtifact::SelfLookup => Ok(ingest_cursor_session(session_id, sink)),
            _ => Err(TapError::Other(
                "cursor tap only supports TapArtifact::SelfLookup".into(),
            )),
        }
    }
}

fn ingest_cursor_session(session_id: &str, sink: &mut dyn TurnSink) -> TapSummary {
    let mut summary = TapSummary::default();
    let bubbles = read_bubbles(session_id);

    if bubbles.is_empty() {
        summary
            .warnings
            .push(format!("cursor: no bubbles for session {session_id}"));
        return summary;
    }

    // Monotonic 0-based index. Idempotent because Cursor's bubble
    // row set is append-only within a session (rows aren't deleted
    // or reordered), so the same scan always yields the same order.
    let mut turn_index: i64 = 0;

    for b in &bubbles {
        let role = match b.btype {
            1 => TurnRole::User,
            2 => TurnRole::Assistant,
            _ => {
                summary.turns_skipped += 1;
                continue;
            }
        };

        let tokens = TurnTokens {
            input: positive_or_none(b.input_tokens),
            cache_read: None,
            cache_creation: None,
            output: positive_or_none(b.output_tokens),
        };

        let tool_names = b.tool_name.clone();
        let tool_call_count = i64::from(b.tool_name.is_some());
        let preview = extract_preview(b);

        let raw_ref = format!("cursor-bubble:{session_id}#{turn_index}");

        let turn = Turn {
            id: Turn::deterministic_id(SOURCE, session_id, turn_index),
            session_id: session_id.to_string(),
            source: SOURCE.to_string(),
            turn_index,
            role,
            started_at: b.created_at_ms,
            ended_at: b.created_at_ms,
            // Cursor doesn't persist per-call model. Downstream may
            // enrich via session-level metadata; we leave it None.
            model: None,
            tokens,
            cost_usd: None,
            tool_call_count,
            thinking_ms: None,
            message_preview: preview,
            raw_ref: Some(raw_ref),
            tool_names,
        };

        sink.accept_turn(turn);
        summary.turns_emitted += 1;
        turn_index += 1;
    }

    if summary.turns_emitted == 0 {
        summary
            .warnings
            .push(format!("cursor: no usable bubbles in session {session_id}"));
    }

    summary
}

/// Cursor occasionally records `-1` or `0` for rows it didn't bill
/// (e.g. meta events). Treat these as "no signal" rather than zero
/// to keep view aggregates honest.
fn positive_or_none(n: i64) -> Option<i64> {
    if n > 0 {
        Some(n)
    } else {
        None
    }
}

fn extract_preview(b: &CursorBubble) -> Option<String> {
    let base = b
        .text
        .as_deref()
        .or(b.thinking.as_deref())
        .unwrap_or("")
        .trim();
    if base.is_empty() {
        return None;
    }
    let one_line: String = base.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = one_line.chars().collect();
    if chars.len() <= 160 {
        Some(chars.into_iter().collect())
    } else {
        let mut s: String = chars.into_iter().take(157).collect();
        s.push_str("...");
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taps::memory_sink::MemorySink;

    #[test]
    fn rejects_wrong_artifact_type() {
        let mut sink = MemorySink::default();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let err = CursorTurnTap.ingest_session("s1", TapArtifact::File(tmp.path()), &mut sink);
        assert!(err.is_err());
    }

    #[test]
    fn self_lookup_on_missing_session_emits_warning_not_error() {
        // In a CI environment without a cursor DB, read_bubbles
        // returns empty and the tap surfaces a warning.
        let mut sink = MemorySink::default();
        let summary = CursorTurnTap
            .ingest_session(
                "nonexistent-session-id-xyz",
                TapArtifact::SelfLookup,
                &mut sink,
            )
            .unwrap();
        assert_eq!(summary.turns_emitted, 0);
        assert!(!summary.warnings.is_empty());
        assert!(sink.turns.is_empty());
    }

    #[test]
    fn positive_or_none_rejects_sentinel_values() {
        assert_eq!(positive_or_none(0), None);
        assert_eq!(positive_or_none(-1), None);
        assert_eq!(positive_or_none(1), Some(1));
        assert_eq!(positive_or_none(999_999), Some(999_999));
    }

    #[test]
    fn preview_collapses_whitespace_and_caps_at_160() {
        let b = CursorBubble {
            btype: 2,
            text: Some("x".repeat(500)),
            thinking: None,
            input_tokens: 0,
            output_tokens: 0,
            created_at_ms: None,
            tool_name: None,
            tool_result: None,
        };
        let p = extract_preview(&b).unwrap();
        assert_eq!(p.chars().count(), 160);
        assert!(p.ends_with("..."));
    }
}
