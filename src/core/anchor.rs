use serde::{Deserialize, Serialize};

/// Whether redacted session transcripts are included alongside anchor metadata
/// on the orphan branch. Anchor metadata is **always** written to the branch
/// (unless the project is ignored); this flag only controls transcripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum TransparencyMode {
    /// Metadata only — no transcripts on the orphan branch.
    #[default]
    Off,
    /// Metadata + redacted transcripts on the orphan branch.
    On,
}

/// How a session was linked to a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkType {
    /// From lifecycle hooks — we know for certain.
    Explicit,
    /// From time-window matching — best guess.
    Inferred,
}

/// Who authored the commit and how AI was involved.
///
/// - `Agent`: AI autonomously committed (non-interactive terminal + active session)
/// - `Assisted`: Human committed while collaborating with AI (interactive + active session)
/// - `Human`: Human committed with no AI involvement
/// - `Automated`: CI/CD or script (non-interactive, no AI session)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum AuthorType {
    Agent,
    Assisted,
    #[default]
    Human,
    Automated,
}

/// A contributor to a commit — human or AI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contributor {
    pub name: String,
    pub role: ContributorRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContributorRole {
    Human,
    Agent,
}

/// How a file's changes were authored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAttribution {
    /// AI agent wrote/edited this file (file appears in tool's files_touched).
    Ai,
    /// Human wrote this file (no AI session touched it).
    Human,
    /// Both AI and human contributed (AI session active but file not in files_touched).
    Mixed,
}

/// Per-file change metadata within a commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    #[serde(alias = "lines_added")]
    pub added: u32,
    #[serde(alias = "lines_deleted")]
    pub deleted: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<FileAttribution>,
    /// Which agent touched this file (if attribution is ai or mixed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// Anchor metadata — the enriched commit primitive.
/// One per commit, stored on the orphan branch and in local SQLite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Anchor {
    pub oobo_version: String,
    pub commit_hash: String,
    pub branch: String,
    /// Git author (always the human who owns the repo).
    pub author: String,
    #[serde(default)]
    pub author_type: AuthorType,
    /// All contributors to this commit — human(s) and AI tool(s).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contributors: Vec<Contributor>,
    pub committed_at: i64,
    pub message: String,

    pub files_changed: Vec<String>,
    #[serde(alias = "lines_added")]
    pub added: u32,
    #[serde(alias = "lines_deleted")]
    pub deleted: u32,

    /// Per-file breakdown with line counts and AI/human attribution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_changes: Vec<FileChange>,

    #[serde(default, alias = "ai_lines_added")]
    pub ai_added: u32,
    #[serde(default, alias = "ai_lines_deleted")]
    pub ai_deleted: u32,
    #[serde(default, alias = "human_lines_added")]
    pub human_added: u32,
    #[serde(default, alias = "human_lines_deleted")]
    pub human_deleted: u32,
    /// AI contribution percentage (0.0–100.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_percentage: Option<f64>,

    pub session_ids: Vec<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,

    pub transparency_mode: TransparencyMode,
}

/// Per-session metadata attached to an anchor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionLink {
    pub session_id: String,
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub link_type: LinkType,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_touched: Option<Vec<String>>,

    #[serde(default)]
    pub is_subagent: bool,
}

impl Anchor {
    pub fn oobo_version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anchor_serialize_roundtrip() {
        let anchor = Anchor {
            oobo_version: "0.1.0".into(),
            commit_hash: "abc123".into(),
            branch: "main".into(),
            author: "Test <test@test.com>".into(),
            author_type: AuthorType::Assisted,
            contributors: vec![
                Contributor {
                    name: "Test <test@test.com>".into(),
                    role: ContributorRole::Human,
                    model: None,
                },
                Contributor {
                    name: "cursor".into(),
                    role: ContributorRole::Agent,
                    model: Some("claude-sonnet-4-20250514".into()),
                },
            ],
            committed_at: 1700000000,
            message: "test commit".into(),
            files_changed: vec!["src/main.rs".into()],
            added: 10,
            deleted: 3,
            file_changes: vec![FileChange {
                path: "src/main.rs".into(),
                added: 10,
                deleted: 3,
                attribution: Some(FileAttribution::Ai),
                agent: Some("cursor".into()),
            }],
            ai_added: 10,
            ai_deleted: 3,
            human_added: 0,
            human_deleted: 0,
            ai_percentage: Some(100.0),
            session_ids: vec!["sess-1".into()],
            summary: Some("Test summary".into()),
            intent: None,
            reasoning: None,
            transparency_mode: TransparencyMode::Off,
        };
        let json = serde_json::to_string(&anchor).unwrap();
        let restored: Anchor = serde_json::from_str(&json).unwrap();
        assert_eq!(anchor.oobo_version, restored.oobo_version);
        assert_eq!(anchor.commit_hash, restored.commit_hash);
        assert_eq!(anchor.branch, restored.branch);
        assert_eq!(anchor.author, restored.author);
        assert_eq!(anchor.author_type, restored.author_type);
        assert_eq!(anchor.contributors, restored.contributors);
        assert_eq!(anchor.committed_at, restored.committed_at);
        assert_eq!(anchor.message, restored.message);
        assert_eq!(anchor.files_changed, restored.files_changed);
        assert_eq!(anchor.added, restored.added);
        assert_eq!(anchor.deleted, restored.deleted);
        assert_eq!(anchor.file_changes.len(), restored.file_changes.len());
        assert_eq!(anchor.ai_added, restored.ai_added);
        assert_eq!(anchor.ai_percentage, restored.ai_percentage);
        assert_eq!(anchor.session_ids, restored.session_ids);
        assert_eq!(anchor.summary, restored.summary);
        assert_eq!(anchor.intent, restored.intent);
        assert_eq!(anchor.reasoning, restored.reasoning);
        assert_eq!(anchor.transparency_mode, restored.transparency_mode);
    }

    #[test]
    fn test_contributors_serialize() {
        let contributors = vec![
            Contributor {
                name: "alice".into(),
                role: ContributorRole::Human,
                model: None,
            },
            Contributor {
                name: "cursor".into(),
                role: ContributorRole::Agent,
                model: Some("gpt-4o".into()),
            },
        ];
        let json = serde_json::to_string(&contributors).unwrap();
        assert!(json.contains("\"human\""));
        assert!(json.contains("\"agent\""));
        assert!(json.contains("\"gpt-4o\""));
        let restored: Vec<Contributor> = serde_json::from_str(&json).unwrap();
        assert_eq!(contributors, restored);
    }

    #[test]
    fn test_anchor_without_contributors_deserializes() {
        let json = r#"{
            "oobo_version": "0.1.0",
            "commit_hash": "abc",
            "branch": "main",
            "author": "test",
            "author_type": "human",
            "committed_at": 0,
            "message": "msg",
            "files_changed": [],
            "lines_added": 0,
            "lines_deleted": 0,
            "session_ids": [],
            "transparency_mode": "off"
        }"#;
        let anchor: Anchor = serde_json::from_str(json).unwrap();
        assert!(anchor.contributors.is_empty());
        assert!(anchor.file_changes.is_empty());
        assert_eq!(anchor.ai_added, 0);
        assert_eq!(anchor.ai_percentage, None);
    }

    #[test]
    fn test_session_link_serialize_minimal() {
        let link = SessionLink {
            session_id: "sess-1".into(),
            agent: "cursor".into(),
            model: None,
            link_type: LinkType::Explicit,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            duration_secs: None,
            tool_calls: None,
            files_touched: None,
            is_subagent: false,
        };
        let json = serde_json::to_string(&link).unwrap();
        assert!(!json.contains("input_tokens"));
    }

    #[test]
    fn test_transparency_mode_serde() {
        assert_eq!(
            serde_json::to_string(&TransparencyMode::Off).unwrap(),
            "\"off\""
        );
        assert_eq!(
            serde_json::to_string(&TransparencyMode::On).unwrap(),
            "\"on\""
        );
    }

    #[test]
    fn test_link_type_serde() {
        assert_eq!(
            serde_json::to_string(&LinkType::Explicit).unwrap(),
            "\"explicit\""
        );
        assert_eq!(
            serde_json::to_string(&LinkType::Inferred).unwrap(),
            "\"inferred\""
        );
    }

    #[test]
    fn test_anchor_oobo_version() {
        assert!(!Anchor::oobo_version().is_empty());
    }

    #[test]
    fn test_author_type_agent_serializes() {
        assert_eq!(
            serde_json::to_string(&AuthorType::Agent).unwrap(),
            "\"agent\""
        );
    }

    #[test]
    fn test_author_type_human_serializes() {
        assert_eq!(
            serde_json::to_string(&AuthorType::Human).unwrap(),
            "\"human\""
        );
    }

    #[test]
    fn test_author_type_automated_serializes() {
        assert_eq!(
            serde_json::to_string(&AuthorType::Automated).unwrap(),
            "\"automated\""
        );
    }

    #[test]
    fn test_author_type_assisted_serializes() {
        assert_eq!(
            serde_json::to_string(&AuthorType::Assisted).unwrap(),
            "\"assisted\""
        );
    }

    #[test]
    fn test_author_type_roundtrip() {
        for variant in [
            AuthorType::Agent,
            AuthorType::Assisted,
            AuthorType::Human,
            AuthorType::Automated,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let restored: AuthorType = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, restored);
        }
    }

    #[test]
    fn test_author_type_default_is_human() {
        assert_eq!(AuthorType::default(), AuthorType::Human);
    }
}
