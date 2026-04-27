use crate::core::anchor::{Anchor, SessionLink};

use super::transcripts::CollectedTranscript;

pub(in crate::git) fn persist_anchor_local(
    _project_root: &str,
    anchor: &Anchor,
    _session_links: &[SessionLink],
) {
    if let Err(errors) = anchor.validate() {
        eprintln!(
            "anchor: warning: anchor invariant check failed for {}: {}",
            anchor.commit_hash,
            errors.join("; ")
        );
    }
}

pub(in crate::git) fn persist_anchor_portable(
    project_root: &str,
    anchor: &Anchor,
    session_links: &[SessionLink],
    transcripts: &[CollectedTranscript],
) {
    if let Err(e) = super::orphan::write_anchor(project_root, anchor, session_links, transcripts) {
        eprintln!("anchor: warning: could not write anchor to orphan branch: {e}");
    }
    super::anchor_cache::invalidate(project_root);
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::anchor::{
        AuthorType, Contributor, ContributorRole, FileAttribution, FileChange,
    };

    #[allow(dead_code)]
    fn anchor() -> Anchor {
        Anchor {
            anchor_schema_version: Anchor::schema_version(),
            oobo_version: Anchor::oobo_version().to_string(),
            commit_hash: "abc123".to_string(),
            branch: "main".to_string(),
            author: "Human <h@example.com>".to_string(),
            author_type: AuthorType::Assisted,
            contributors: vec![
                Contributor {
                    name: "Human <h@example.com>".to_string(),
                    role: ContributorRole::Human,
                    model: None,
                },
                Contributor {
                    name: "claude".to_string(),
                    role: ContributorRole::Agent,
                    model: Some("sonnet".to_string()),
                },
            ],
            committed_at: 1_700_000_000,
            message: "test commit".to_string(),
            files_changed: vec!["src/lib.rs".to_string()],
            added: 4,
            deleted: 1,
            file_changes: vec![FileChange {
                path: "src/lib.rs".to_string(),
                added: 4,
                deleted: 1,
                attribution: Some(FileAttribution::Mixed),
                agent: Some("claude".to_string()),
                line_attributions: Vec::new(),
            }],
            ai_added: 3,
            ai_deleted: 0,
            human_added: 1,
            human_deleted: 1,
            ai_percentage: Some(60.0),
            session_ids: vec!["s1".to_string()],
            summary: None,
            intent: None,
            reasoning: None,
            transparency_mode: crate::core::anchor::TransparencyMode::Off,
            file_interactions: None,
            turns: Vec::new(),
        }
    }
}
