use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::anchor::{Anchor, SessionLink};

pub const EVENT_PAYLOAD_SCHEMA_VERSION: u32 = 1;

fn default_event_payload_schema_version() -> u32 {
    EVENT_PAYLOAD_SCHEMA_VERSION
}

/// Anchor payload envelope — kept for schema documentation and test round-trips.
#[derive(Debug, Serialize, Deserialize)]
pub struct EventPayload {
    /// Version of the remote transport envelope. The embedded anchor has its
    /// own schema version and remains the canonical commit-memory object.
    #[serde(default = "default_event_payload_schema_version")]
    pub payload_schema_version: u32,
    pub event: String,
    pub timestamp: DateTime<Utc>,
    pub oobo_version: String,
    pub project: ProjectInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<AnchorPayload>,
    /// Flat transcript messages (backward compat — all sessions concatenated).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transcript: Vec<TranscriptMessage>,
    /// Structured per-session transcripts with parent-child relationships.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub session_transcripts: Vec<SessionTranscript>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    pub git_remote: Option<String>,
}

/// The anchor plus its linked sessions, mirroring what's on the orphan branch.
#[derive(Debug, Serialize, Deserialize)]
pub struct AnchorPayload {
    #[serde(flatten)]
    pub anchor: Anchor,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<SessionLink>,
}

/// A tool invocation within a transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallMessage {
    pub tool_use_id: String,
    pub name: String,
    pub input_summary: String,
}

/// The result of a tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultMessage {
    pub tool_use_id: String,
    pub name: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_summary: Option<String>,
}

/// A single transcript message. All optional fields are additive — older
/// backends that only read `role` + `text` continue to work unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<ToolCallMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolResultMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_ms: Option<i64>,
}

/// A session's transcript with parent-child relationship metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTranscript {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_type: Option<String>,
    pub messages: Vec<TranscriptMessage>,
}

/// SessionStats is still used internally by the analytics pipeline
/// for computing and storing session-level token counts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionStats {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u64>,
    #[serde(default)]
    pub is_estimated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_touched: Vec<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tool_call_count: u32,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// Error body returned by the remote API on 4xx / 5xx (e.g. 401 from search).
#[derive(Debug, Deserialize)]
pub struct IngestError {
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub success: Option<bool>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<SearchProjectScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchProjectScope {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchResponse {
    #[serde(default)]
    pub answer: Option<String>,
    #[serde(default)]
    pub hits: Vec<SearchHit>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchHit {
    #[serde(default)]
    pub project: SearchProject,
    #[serde(default)]
    pub anchor_sha: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    /// Memory hits use plural `session_ids` instead of `session_id`.
    #[serde(default)]
    pub session_ids: Option<Vec<String>>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub tokens: Option<i64>,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub snippet: Option<String>,
    #[serde(default)]
    pub score: Option<f64>,
    /// `"fts"` for full-text search hits, `"memory"` for semantic memory hits.
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub memory_id: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SearchProject {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

// ── Delta (textual diff between anchors) ──────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct DeltaRequest {
    pub anchor_sha: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_remote: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_id: Option<String>,
    #[serde(default)]
    pub full: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaResponse {
    #[serde(default)]
    pub current: Option<DeltaAnchorSummary>,
    #[serde(default)]
    pub previous: Option<DeltaAnchorSummary>,
    #[serde(default)]
    pub changes: Option<DeltaChanges>,
    #[serde(default)]
    pub current_detail: Option<serde_json::Value>,
    #[serde(default)]
    pub previous_detail: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaAnchorSummary {
    #[serde(default)]
    pub sha: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub headline: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub complexity: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaChanges {
    #[serde(default)]
    pub category_shift: Option<DeltaShift>,
    #[serde(default)]
    pub complexity_shift: Option<DeltaShift>,
    #[serde(default)]
    pub new_areas: Vec<String>,
    #[serde(default)]
    pub new_techniques: Vec<String>,
    #[serde(default)]
    pub files_new: Vec<String>,
    #[serde(default)]
    pub files_continued: Vec<String>,
    #[serde(default)]
    pub narrative: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaShift {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeltaErrorResponse {
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::anchor::*;

    #[test]
    fn test_serialize_commit_event() {
        let anchor = Anchor {
            anchor_schema_version: ANCHOR_SCHEMA_VERSION,
            oobo_version: "0.1.0".into(),
            commit_hash: "abc123".into(),
            branch: "main".into(),
            author: "Dev <dev@co.com>".into(),
            author_type: AuthorType::Assisted,
            contributors: vec![
                Contributor {
                    name: "Dev <dev@co.com>".into(),
                    role: ContributorRole::Human,
                    model: None,
                },
                Contributor {
                    name: "claude".into(),
                    role: ContributorRole::Agent,
                    model: Some("claude-sonnet-4".into()),
                },
            ],
            committed_at: 1773282899,
            message: "feat: add auth".into(),
            files_changed: vec!["auth.js".into()],
            added: 35,
            deleted: 0,
            file_changes: vec![FileChange {
                path: "auth.js".into(),
                added: 24,
                deleted: 0,
                attribution: Some(FileAttribution::Ai),
                agent: Some("claude".into()),
                line_attributions: Vec::new(),
            }],
            ai_added: 24,
            ai_deleted: 0,
            human_added: 11,
            human_deleted: 0,
            ai_percentage: Some(68.57),
            session_ids: vec!["sess-1".into()],
            summary: None,
            intent: None,
            reasoning: None,
            transparency_mode: TransparencyMode::On,
            file_interactions: None,
            turns: vec![AnchorTurnRef {
                id: "turn-1".into(),
                session_id: "sess-1".into(),
                source: "claude".into(),
                turn_index: 0,
                tree_hash: Some("tree123".into()),
            }],
        };

        let session_links = vec![SessionLink {
            session_id: "sess-1".into(),
            agent: "claude".into(),
            model: Some("claude-sonnet-4".into()),
            link_type: LinkType::Explicit,
            input_tokens: Some(15000),
            output_tokens: Some(8000),
            cache_read_tokens: None,
            cache_creation_tokens: None,
            duration_secs: Some(120),
            tool_calls: Some(5),
            files_touched: Some(vec!["auth.js".into()]),
            tool_usage: None,
            tool_failures: None,
            subagent_count: None,
            bash_commands: None,
            thinking_duration_ms: None,
            compact_count: None,
            context_tokens: None,
            context_window_size: None,
            is_subagent: false,
            parent_session_id: None,
            subagent_type: None,
            is_estimated: false,
            peer_session_ids: Vec::new(),
        }];

        let payload = EventPayload {
            payload_schema_version: EVENT_PAYLOAD_SCHEMA_VERSION,
            event: "git.commit".into(),
            timestamp: chrono::Utc::now(),
            oobo_version: "0.1.0".into(),
            project: ProjectInfo {
                name: "my-app".into(),
                git_remote: Some("github.com/user/my-app".into()),
            },
            anchor: Some(AnchorPayload {
                anchor,
                sessions: session_links,
            }),
            transcript: vec![
                TranscriptMessage {
                    role: "user".into(),
                    text: Some("Add auth flow".into()),
                    thinking: None,
                    tool_call: None,
                    tool_result: None,
                    timestamp_ms: None,
                },
                TranscriptMessage {
                    role: "assistant".into(),
                    text: Some("I'll create the auth module...".into()),
                    thinking: None,
                    tool_call: None,
                    tool_result: None,
                    timestamp_ms: None,
                },
            ],
            session_transcripts: Vec::new(),
        };

        let json = serde_json::to_string_pretty(&payload).unwrap();
        assert!(json.contains("payload_schema_version"));
        assert!(json.contains("anchor_schema_version"));
        assert!(json.contains("git.commit"));
        assert!(json.contains("abc123"));
        assert!(json.contains("assisted"));
        assert!(json.contains("ai_percentage"));
        assert!(json.contains("68.57"));
        assert!(json.contains("claude"));
        assert!(json.contains("auth.js"));
        assert!(json.contains("github.com/user/my-app"));
        assert!(json.contains("Add auth flow"));
        assert!(json.contains("turn-1"));
        assert!(!json.contains("hook_events"));
        assert!(!json.contains("native_transcript_path"));
        assert!(!json.contains("cost"));
    }

    #[test]
    fn test_serialize_push_event_no_anchor() {
        let payload = EventPayload {
            payload_schema_version: EVENT_PAYLOAD_SCHEMA_VERSION,
            event: "git.push".into(),
            timestamp: chrono::Utc::now(),
            oobo_version: "0.1.0".into(),
            project: ProjectInfo {
                name: "my-app".into(),
                git_remote: None,
            },
            anchor: None,
            transcript: Vec::new(),
            session_transcripts: Vec::new(),
        };

        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("git.push"));
        assert!(!json.contains("anchor"));
        assert!(!json.contains("transcript"));
    }

    #[test]
    fn test_payload_schema_version_defaults_for_legacy_json() {
        let json = serde_json::json!({
            "event": "git.push",
            "timestamp": chrono::Utc::now(),
            "oobo_version": "0.1.0",
            "project": {
                "name": "my-app",
                "git_remote": null
            }
        });

        let payload: EventPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.payload_schema_version, EVENT_PAYLOAD_SCHEMA_VERSION);
    }
}
