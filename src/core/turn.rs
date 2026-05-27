//! The atomic unit of AI capture.
//!
//! A [`Turn`] represents a single model invocation within a session  -- 
//! one user→assistant exchange plus any tool-call activity that
//! happened in that exchange. Tokens recorded on a turn are **deltas**
//! (exactly what the model API billed for this call), never cumulative.
//!
//! This is the ground truth from which every token, cost, and
//! attribution view in oobo derives. Higher layers (sessions, anchor
//! contributions, project totals) are rollups over `turns`.
//!
//! Extensibility: adding support for a new AI tool means writing one
//! [`TurnTap`] that maps that tool's native artifact onto a stream of
//! `Turn` values. The rest of the pipeline does not change.

use serde::{Deserialize, Serialize};

/// The role the turn represents in the conversation.
///
/// This mirrors the OpenAI/Anthropic role taxonomy; tools that don't
/// have all roles should use [`TurnRole::Assistant`] for agent output
/// and [`TurnRole::Tool`] for tool_result entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnRole {
    User,
    Assistant,
    Tool,
    System,
}

impl TurnRole {
    #[cfg(test)]
    pub fn as_str(self) -> &'static str {
        match self {
            TurnRole::User => "user",
            TurnRole::Assistant => "assistant",
            TurnRole::Tool => "tool",
            TurnRole::System => "system",
        }
    }

    #[cfg(test)]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(TurnRole::User),
            "assistant" => Some(TurnRole::Assistant),
            "tool" => Some(TurnRole::Tool),
            "system" => Some(TurnRole::System),
            _ => None,
        }
    }
}

/// A single model invocation within a session.
///
/// Invariants:
/// - `(session_id, source, turn_index)` is globally unique.
/// - Token counts, when populated, are exactly the values reported by
///   the model API for **this single call**  --  not cumulative totals.
/// - `id` is deterministic (see [`Turn::deterministic_id`]) so
///   re-ingesting the same artifact is idempotent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    /// Deterministic id, stable across re-ingestion of the same artifact.
    pub id: String,

    /// Session this turn belongs to. Matches `sessions.(id, source)`.
    pub session_id: String,
    pub source: String,

    /// 0-based monotonic index within the session.
    pub turn_index: i64,

    pub role: TurnRole,

    /// When the model began producing output (best-effort: some tools
    /// only expose one timestamp; then `started_at == ended_at`).
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,

    pub model: Option<String>,

    /// Per-call token deltas, exactly as reported by the model.
    pub tokens: TurnTokens,

    /// Cost in USD derived from [`tokens`] and the model's pricing
    /// table. Populated by the tap layer when pricing is known.
    pub cost_usd: Option<f64>,

    /// Number of tool calls that happened in this turn. Used to roll
    /// up per-anchor tool activity without cross-joining `turns` to
    /// a tool_calls table.
    pub tool_call_count: i64,

    /// Milliseconds spent in hidden reasoning (Claude "thinking").
    pub thinking_ms: Option<i64>,

    /// Redacted preview (short snippet) of the user or assistant
    /// content for TUI / search display. Never contains secrets;
    /// redaction policy lives in `src/redact`.
    pub message_preview: Option<String>,

    /// Pointer back to the tool-native artifact (e.g. `"jsonl:<path>#42"`
    /// for Claude transcripts, `"composer:<uuid>#3"` for Cursor).
    /// Allows "open the raw source" affordances in the UI without
    /// keeping raw content in the database.
    pub raw_ref: Option<String>,

    /// Comma-joined list of tool_use names that fired on this turn,
    /// in invocation order, e.g. `"Read,Write,Task"`. `None` means the
    /// tap didn't extract tool metadata (old artifact, unsupported
    /// source); `Some("")` would be unusual but permitted.
    ///
    /// This is the hook the M4 subagent inference engine uses to find
    /// parent turns that spawned subagents  --  `Task` presence is the
    /// strongest signal in our heuristic stack.
    pub tool_names: Option<String>,
}

/// Per-call token deltas. Every number is the value the API billed
/// for **this specific call**  --  not a cumulative sum across the session.
///
/// Field semantics (Anthropic & compatible vendors; OpenAI maps nearly 1:1):
/// - `input`: non-cached prompt tokens on this call
/// - `cache_read`: cached prompt tokens read on this call
/// - `cache_creation`: tokens added to the prompt cache on this call
/// - `output`: generated tokens (assistant content) on this call
///
/// For tools that don't expose cache metrics, `cache_read` and
/// `cache_creation` stay `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnTokens {
    pub input: Option<i64>,
    pub cache_read: Option<i64>,
    pub cache_creation: Option<i64>,
    pub output: Option<i64>,
}

pub const TURN_SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// A restorable point in an agent session.
///
/// `Turn` remains the normalized accounting row used for token/cost rollups.
/// `TurnSnapshot` is the Git-backed memory object: worktree state, lineage, and
/// full native memory captured at an agent turn boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnSnapshot {
    pub schema_version: u32,
    pub id: String,
    pub project_id: String,
    pub worktree_id: String,
    pub session_id: String,
    pub source: String,
    pub turn_index: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub files: Vec<TurnFileSnapshot>,
    #[serde(default)]
    pub memory: TurnMemoryPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnFileSnapshot {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_blob: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_blob: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TurnMemoryPayload {
    /// Tool-native transcript path when the tool exposes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    /// Full native transcript slice or session payload for this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<serde_json::Value>,
    /// Hook events observed while this turn was active.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hook_events: Vec<TurnHookEvent>,
    /// Tool calls observed while this turn was active.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<TurnToolCall>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnHookEvent {
    pub name: String,
    pub observed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnToolCall {
    pub name: String,
    pub observed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
    #[serde(default)]
    pub failed: bool,
}

impl TurnTokens {
    #[cfg(test)]
    pub fn billed(&self) -> i64 {
        self.input.unwrap_or(0)
            + self.cache_read.unwrap_or(0)
            + self.cache_creation.unwrap_or(0)
            + self.output.unwrap_or(0)
    }

    #[cfg(test)]
    pub fn new_work(&self) -> i64 {
        self.output.unwrap_or(0) + self.cache_creation.unwrap_or(0)
    }

    #[cfg(test)]
    pub fn context(&self) -> i64 {
        self.input.unwrap_or(0) + self.cache_read.unwrap_or(0)
    }

    #[cfg(test)]
    pub fn has_any(&self) -> bool {
        self.input.is_some()
            || self.cache_read.is_some()
            || self.cache_creation.is_some()
            || self.output.is_some()
    }
}

impl Turn {
    /// Produce a deterministic id for a turn.
    ///
    /// The DB already enforces uniqueness via `UNIQUE(session_id,
    /// source, turn_index)`, so this value is purely for external
    /// references (URIs, logs, orphan-branch payloads). We use a
    /// 64-bit FNV-1a hash rendered as 16 hex chars  --  short, readable,
    /// and collision-free at workstation scale.
    pub fn deterministic_id(source: &str, session_id: &str, turn_index: i64) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let prime: u64 = 0x0000_0100_0000_01B3;
        for b in source.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(prime);
        }
        h ^= u64::from(b'|');
        h = h.wrapping_mul(prime);
        for b in session_id.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(prime);
        }
        h ^= u64::from(b'|');
        h = h.wrapping_mul(prime);
        for b in turn_index.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(prime);
        }
        format!("{h:016x}")
    }
}

impl TurnSnapshot {
    pub fn new(
        project_id: &str,
        worktree_id: &str,
        source: &str,
        session_id: &str,
        turn_index: i64,
    ) -> Self {
        Self {
            schema_version: TURN_SNAPSHOT_SCHEMA_VERSION,
            id: Self::stable_id(project_id, worktree_id, source, session_id, turn_index),
            project_id: project_id.to_string(),
            worktree_id: worktree_id.to_string(),
            session_id: session_id.to_string(),
            source: source.to_string(),
            turn_index,
            parent_id: None,
            restored_from: None,
            base_commit: None,
            head_commit: None,
            tree_hash: None,
            branch: None,
            created_at: chrono::Utc::now().timestamp(),
            started_at: None,
            ended_at: None,
            model: None,
            files: Vec::new(),
            memory: TurnMemoryPayload::default(),
        }
    }

    pub fn stable_id(
        project_id: &str,
        worktree_id: &str,
        source: &str,
        session_id: &str,
        turn_index: i64,
    ) -> String {
        let input = format!("{project_id}|{worktree_id}|{source}|{session_id}|{turn_index}");
        format!("t{:016x}", fnv1a64(input.as_bytes()))
    }
}

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let prime: u64 = 0x0000_0100_0000_01B3;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(prime);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn billed_is_sum_of_all_four_components() {
        let t = TurnTokens {
            input: Some(10),
            cache_read: Some(20),
            cache_creation: Some(30),
            output: Some(40),
        };
        assert_eq!(t.billed(), 100);
    }

    #[test]
    fn billed_treats_missing_as_zero_not_noise() {
        let t = TurnTokens {
            output: Some(5),
            ..Default::default()
        };
        assert_eq!(t.billed(), 5);
        assert_eq!(t.context(), 0);
        assert_eq!(t.new_work(), 5);
    }

    #[test]
    fn new_work_excludes_cache_read_and_input() {
        // Even a huge cached context with tiny output yields tiny new_work.
        let t = TurnTokens {
            input: Some(10),
            cache_read: Some(100_000),
            cache_creation: Some(0),
            output: Some(50),
        };
        assert_eq!(t.new_work(), 50);
        assert_eq!(t.billed(), 100_060);
    }

    #[test]
    fn has_any_is_false_only_when_completely_empty() {
        assert!(!TurnTokens::default().has_any());
        assert!(TurnTokens {
            input: Some(0),
            ..Default::default()
        }
        .has_any());
    }

    #[test]
    fn deterministic_id_is_stable_and_position_sensitive() {
        let a = Turn::deterministic_id("claude", "sess-1", 0);
        let b = Turn::deterministic_id("claude", "sess-1", 0);
        assert_eq!(a, b, "same inputs → same id");

        let c = Turn::deterministic_id("claude", "sess-1", 1);
        assert_ne!(a, c, "different turn_index → different id");

        let d = Turn::deterministic_id("cursor", "sess-1", 0);
        assert_ne!(a, d, "different source → different id");
    }

    #[test]
    fn snapshot_id_is_stable_and_scoped_to_worktree() {
        let a = TurnSnapshot::stable_id("p:repo", "wt1", "cursor", "s1", 3);
        let b = TurnSnapshot::stable_id("p:repo", "wt1", "cursor", "s1", 3);
        let c = TurnSnapshot::stable_id("p:repo", "wt2", "cursor", "s1", 3);

        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with('t'));
    }

    #[test]
    fn snapshot_new_sets_schema_and_identity() {
        let snapshot = TurnSnapshot::new("p:repo", "wt1", "claude", "session", 7);
        assert_eq!(snapshot.schema_version, TURN_SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(snapshot.project_id, "p:repo");
        assert_eq!(snapshot.worktree_id, "wt1");
        assert_eq!(snapshot.source, "claude");
        assert_eq!(snapshot.session_id, "session");
        assert_eq!(snapshot.turn_index, 7);
    }

    #[test]
    fn role_round_trips() {
        for r in [
            TurnRole::User,
            TurnRole::Assistant,
            TurnRole::Tool,
            TurnRole::System,
        ] {
            assert_eq!(TurnRole::parse(r.as_str()), Some(r));
        }
        assert_eq!(TurnRole::parse("bogus"), None);
    }
}
