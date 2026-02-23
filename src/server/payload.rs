use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct EventPayload {
    pub event: String,
    pub timestamp: DateTime<Utc>,
    pub project: ProjectInfo,
    pub git: GitInfo,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tools: BTreeMap<String, ToolContext>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub root: String,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GitInfo {
    pub operation: String,
    pub branch: String,
    pub commit_hash: String,
    pub commit_message: String,
    pub author: String,
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContext {
    pub active_sessions: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_session: Option<SessionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub name: String,
    pub mode: String,
    pub message_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<SessionStats>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionStats {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
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

/// Collect ToolContext for a given tool's sessions.
pub fn tool_context_from_sessions(
    sessions: &[crate::cursor::Session],
    count_fn: impl Fn(&str, &str) -> u32,
) -> Option<ToolContext> {
    if sessions.is_empty() {
        return None;
    }
    let recent = &sessions[0];
    let msg_count = count_fn(&recent.project_path, &recent.session_id);
    Some(ToolContext {
        active_sessions: sessions.len() as u32,
        recent_session: Some(SessionSummary {
            id: recent.session_id.clone(),
            name: recent.name.clone(),
            mode: recent.mode.clone(),
            message_count: msg_count,
            stats: None,
        }),
    })
}

/// Collect ToolContext with stats for a given tool's sessions.
pub fn tool_context_from_sessions_with_stats(
    sessions: &[crate::cursor::Session],
    count_fn: impl Fn(&str, &str) -> u32,
    stats_fn: impl Fn(&str, &str) -> Option<SessionStats>,
) -> Option<ToolContext> {
    if sessions.is_empty() {
        return None;
    }
    let recent = &sessions[0];
    let msg_count = count_fn(&recent.project_path, &recent.session_id);
    let stats = stats_fn(&recent.project_path, &recent.session_id);
    Some(ToolContext {
        active_sessions: sessions.len() as u32,
        recent_session: Some(SessionSummary {
            id: recent.session_id.clone(),
            name: recent.name.clone(),
            mode: recent.mode.clone(),
            message_count: msg_count,
            stats,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_payload() {
        let mut tools = BTreeMap::new();
        tools.insert(
            "cursor".into(),
            ToolContext {
                active_sessions: 2,
                recent_session: Some(SessionSummary {
                    id: "abc-123".into(),
                    name: "Fix auth".into(),
                    mode: "agent".into(),
                    message_count: 15,
                    stats: None,
                }),
            },
        );
        tools.insert(
            "claude".into(),
            ToolContext {
                active_sessions: 1,
                recent_session: Some(SessionSummary {
                    id: "def-456".into(),
                    name: "Refactor DB".into(),
                    mode: "opus-4.5".into(),
                    message_count: 8,
                    stats: Some(SessionStats {
                        model: Some("claude-opus-4-5".into()),
                        input_tokens: Some(15000),
                        output_tokens: Some(8000),
                        total_cost_usd: Some(0.45),
                        duration_secs: Some(120),
                        files_touched: vec!["src/db.rs".into(), "src/main.rs".into()],
                        tool_call_count: 5,
                    }),
                }),
            },
        );

        let payload = EventPayload {
            event: "git.commit".into(),
            timestamp: Utc::now(),
            project: ProjectInfo {
                root: "/home/user/project".into(),
                name: "project".into(),
            },
            git: GitInfo {
                operation: "commit".into(),
                branch: "main".into(),
                commit_hash: "abc123".into(),
                commit_message: "fix bug".into(),
                author: "Test <test@test.com>".into(),
                files_changed: 3,
                insertions: 42,
                deletions: 10,
            },
            tools,
        };

        let json = serde_json::to_string_pretty(&payload).unwrap();
        assert!(json.contains("git.commit"));
        assert!(json.contains("abc123"));
        assert!(json.contains("Fix auth"));
        assert!(json.contains("Refactor DB"));
        assert!(json.contains("\"cursor\""));
        assert!(json.contains("\"claude\""));
    }

    #[test]
    fn test_serialize_without_tools() {
        let payload = EventPayload {
            event: "git.push".into(),
            timestamp: Utc::now(),
            project: ProjectInfo {
                root: "/tmp".into(),
                name: "tmp".into(),
            },
            git: GitInfo {
                operation: "push".into(),
                branch: "main".into(),
                commit_hash: String::new(),
                commit_message: String::new(),
                author: String::new(),
                files_changed: 0,
                insertions: 0,
                deletions: 0,
            },
            tools: BTreeMap::new(),
        };

        let json = serde_json::to_string(&payload).unwrap();
        assert!(!json.contains("tools"));
    }
}
