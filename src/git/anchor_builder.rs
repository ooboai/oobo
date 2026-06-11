use crate::core::anchor::{
    Anchor, AnchorTurnRef, AuthorType, Contributor, ContributorRole, FileChange, FileInteraction,
    SessionLink, TransparencyMode,
};

use super::context::GitContext;

pub(in crate::git) struct AttributionResult {
    pub file_changes: Vec<FileChange>,
    pub ai_added: u32,
    pub ai_deleted: u32,
    pub human_added: u32,
    pub human_deleted: u32,
    pub ai_percentage: Option<f64>,
}

pub(in crate::git) struct AnchorBuildInput<'a> {
    pub branch: &'a str,
    pub git_context: &'a GitContext,
    pub author_type: AuthorType,
    pub session_links: &'a [SessionLink],
    pub files_changed: Vec<String>,
    pub attribution: AttributionResult,
    pub transparency: TransparencyMode,
    pub file_interactions: Vec<FileInteraction>,
    pub turns: Vec<AnchorTurnRef>,
}

pub(in crate::git) fn build_anchor(input: AnchorBuildInput<'_>) -> Anchor {
    let AnchorBuildInput {
        branch,
        git_context,
        author_type,
        session_links,
        files_changed,
        attribution,
        transparency,
        file_interactions,
        turns,
    } = input;

    let mut contributors = vec![Contributor {
        name: git_context.author.clone(),
        role: ContributorRole::Human,
        model: None,
    }];
    for link in session_links {
        if !contributors.iter().any(|c| c.name == link.agent) {
            contributors.push(Contributor {
                name: link.agent.clone(),
                role: ContributorRole::Agent,
                model: link.model.clone(),
            });
        }
    }

    Anchor {
        anchor_schema_version: Anchor::schema_version(),
        oobo_version: Anchor::oobo_version().to_string(),
        commit_hash: git_context.commit_hash.clone(),
        branch: branch.to_string(),
        author: git_context.author.clone(),
        author_type,
        contributors,
        // The commit's own committer timestamp: deterministic across
        // async-worker replays, honest when enrichment runs late.
        committed_at: git_context.committed_at,
        message: git_context.commit_message.clone(),
        files_changed,
        added: git_context.insertions,
        deleted: git_context.deletions,
        file_changes: attribution.file_changes,
        ai_added: attribution.ai_added,
        ai_deleted: attribution.ai_deleted,
        human_added: attribution.human_added,
        human_deleted: attribution.human_deleted,
        ai_percentage: attribution.ai_percentage,
        session_ids: session_links.iter().map(|s| s.session_id.clone()).collect(),
        summary: None,
        intent: None,
        reasoning: None,
        transparency_mode: transparency,
        file_interactions: if file_interactions.is_empty() {
            None
        } else {
            Some(file_interactions)
        },
        turns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::anchor::LinkType;

    fn git_context() -> GitContext {
        GitContext {
            commit_hash: "abc123".to_string(),
            commit_message: "test commit".to_string(),
            author: "Human <h@example.com>".to_string(),
            committed_at: 1_700_000_000,
            files_changed: 1,
            insertions: 3,
            deletions: 1,
        }
    }

    fn link(session_id: &str, agent: &str, model: Option<&str>) -> SessionLink {
        SessionLink {
            session_id: session_id.to_string(),
            agent: agent.to_string(),
            model: model.map(str::to_string),
            link_type: LinkType::Explicit,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            duration_secs: None,
            tool_calls: None,
            files_touched: None,
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
        }
    }

    fn attribution() -> AttributionResult {
        AttributionResult {
            file_changes: Vec::new(),
            ai_added: 2,
            ai_deleted: 0,
            human_added: 1,
            human_deleted: 1,
            ai_percentage: Some(50.0),
        }
    }

    #[test]
    fn build_anchor_deduplicates_agent_contributors() {
        let links = [
            link("s1", "claude", Some("sonnet")),
            link("s2", "claude", Some("sonnet")),
        ];

        let anchor = build_anchor(AnchorBuildInput {
            branch: "main",
            git_context: &git_context(),
            author_type: AuthorType::Assisted,
            session_links: &links,
            files_changed: vec!["src/lib.rs".to_string()],
            attribution: attribution(),
            transparency: TransparencyMode::Off,
            file_interactions: Vec::new(),
            turns: Vec::new(),
        });

        assert_eq!(anchor.session_ids, vec!["s1".to_string(), "s2".to_string()]);
        assert_eq!(anchor.contributors.len(), 2);
        assert_eq!(anchor.contributors[0].role, ContributorRole::Human);
        assert_eq!(anchor.contributors[1].name, "claude");
        assert_eq!(anchor.contributors[1].model.as_deref(), Some("sonnet"));
        assert!(anchor.file_interactions.is_none());
    }

    #[test]
    fn build_anchor_carries_git_and_attribution_fields() {
        let links = [link("s1", "cursor", None)];

        let anchor = build_anchor(AnchorBuildInput {
            branch: "feature",
            git_context: &git_context(),
            author_type: AuthorType::Agent,
            session_links: &links,
            files_changed: vec!["src/main.rs".to_string()],
            attribution: attribution(),
            transparency: TransparencyMode::On,
            file_interactions: Vec::new(),
            turns: Vec::new(),
        });

        assert_eq!(anchor.commit_hash, "abc123");
        assert_eq!(anchor.branch, "feature");
        assert_eq!(anchor.added, 3);
        assert_eq!(anchor.deleted, 1);
        assert_eq!(anchor.ai_added, 2);
        assert_eq!(anchor.human_deleted, 1);
        assert_eq!(anchor.transparency_mode, TransparencyMode::On);
    }
}
