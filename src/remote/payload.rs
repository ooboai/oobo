use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::anchor::{Anchor, SessionLink};

/// Event payload sent to the backend on git write operations.
///
/// The `anchor` field is the same structure written to the orphan branch,
/// so a self-hosted backend reading the orphan branch directly would see
/// identical data. The optional `transcript` is only included when
/// transparency mode is on, and is always redacted through gitleaks.
#[derive(Debug, Serialize, Deserialize)]
pub struct EventPayload {
    pub event: String,
    pub timestamp: DateTime<Utc>,
    pub oobo_version: String,
    pub project: ProjectInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<AnchorPayload>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transcript: Vec<TranscriptMessage>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptMessage {
    pub role: String,
    pub text: String,
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

fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// Successful response from `POST /anchors/ingest`.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct IngestResponse {
    pub success: bool,
    pub message: String,
    #[serde(default)]
    pub data: Option<IngestData>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct IngestData {
    pub anchor_id: String,
    pub commit_hash: String,
}

/// Error body returned by the ingestion API on 4xx / 5xx.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct IngestError {
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub success: Option<bool>,
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
            is_subagent: false,
            is_estimated: false,
        }];

        let payload = EventPayload {
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
                    text: "Add auth flow".into(),
                },
                TranscriptMessage {
                    role: "assistant".into(),
                    text: "I'll create the auth module...".into(),
                },
            ],
        };

        let json = serde_json::to_string_pretty(&payload).unwrap();
        assert!(json.contains("git.commit"));
        assert!(json.contains("abc123"));
        assert!(json.contains("assisted"));
        assert!(json.contains("ai_percentage"));
        assert!(json.contains("68.57"));
        assert!(json.contains("claude"));
        assert!(json.contains("auth.js"));
        assert!(json.contains("github.com/user/my-app"));
        assert!(json.contains("Add auth flow"));
        assert!(!json.contains("cost"));
    }

    #[test]
    fn test_serialize_push_event_no_anchor() {
        let payload = EventPayload {
            event: "git.push".into(),
            timestamp: chrono::Utc::now(),
            oobo_version: "0.1.0".into(),
            project: ProjectInfo {
                name: "my-app".into(),
                git_remote: None,
            },
            anchor: None,
            transcript: Vec::new(),
        };

        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("git.push"));
        assert!(!json.contains("anchor"));
        assert!(!json.contains("transcript"));
    }
}
