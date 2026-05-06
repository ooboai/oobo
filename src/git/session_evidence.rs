use crate::hooks;

/// Match a session's agent name to a tool's canonical name.
/// Handles Cursor's various mode names ("agent", "composer", etc.) which
/// are now normalized at hook time, plus legacy session files with old names.
pub(in crate::git) fn is_agent_tool_match(session_agent: &str, tool_name: &str) -> bool {
    if session_agent == tool_name {
        return true;
    }
    crate::core::tool::is_cursor_agent(session_agent)
        && (tool_name == "composer" || tool_name == "cursor")
}

/// Normalize a file path to be relative to the project root.
pub(in crate::git) fn normalize_path(file_path: &str, project_root: &str) -> String {
    let path = std::path::Path::new(file_path);
    if path.is_absolute() {
        path.strip_prefix(project_root).map_or_else(|_| file_path.to_string(), |p| p.to_string_lossy().to_string())
    } else {
        file_path.to_string()
    }
}

pub(in crate::git) fn ms_to_secs(ms: i64) -> i64 {
    if ms > 1_000_000_000_000 {
        ms / 1000
    } else {
        ms
    }
}

/// Recency-based fallback when file overlap data is unavailable.
pub(in crate::git) fn filter_by_recency(
    sessions: &[hooks::state::ActiveSession],
) -> Vec<hooks::state::ActiveSession> {
    let now = chrono::Utc::now().timestamp();

    if sessions.is_empty() {
        return Vec::new();
    }

    let mut sorted: Vec<&hooks::state::ActiveSession> = sessions.iter().collect();
    sorted.sort_by_key(|s| std::cmp::Reverse(s.updated_at));

    let most_recent = sorted[0].updated_at;

    if now - most_recent < 120 {
        return sorted
            .into_iter()
            .filter(|s| most_recent - s.updated_at < 10)
            .cloned()
            .collect();
    }

    sorted
        .into_iter()
        .filter(|s| now - s.updated_at < 1800)
        .cloned()
        .collect()
}

/// Detect files touched by multiple sessions and return both file interactions
/// and the peer map. Accepts references to avoid cloning `ActiveSession` data.
/// Expects only top-level sessions — subagent sessions are filtered out
/// by the caller using `subagent_runs` data.
pub(in crate::git) fn detect_file_interactions_refs(
    sessions: &[&hooks::state::ActiveSession],
    project_root: &str,
) -> (
    Vec<crate::core::anchor::FileInteraction>,
    std::collections::HashMap<String, Vec<String>>,
) {
    let inputs: Vec<crate::core::anchor::SessionFiles> = sessions
        .iter()
        .map(|s| {
            let (edited, read) = if s.edited_files.is_some() || s.read_files.is_some() {
                (
                    s.edited_files
                        .as_ref()
                        .map(|h| h.iter().cloned().collect())
                        .unwrap_or_default(),
                    s.read_files
                        .as_ref()
                        .map(|h| h.iter().cloned().collect())
                        .unwrap_or_default(),
                )
            } else {
                hooks::state::get_file_sets(project_root, &s.session_id)
            };
            crate::core::anchor::SessionFiles {
                session_id: s.session_id.clone(),
                edited,
                read,
            }
        })
        .collect();

    crate::core::anchor::detect_interactions(&inputs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session(
        id: &str,
        edited: Vec<&str>,
        read: Vec<&str>,
    ) -> crate::hooks::state::ActiveSession {
        let now = chrono::Utc::now().timestamp();
        crate::hooks::state::ActiveSession {
            session_id: id.into(),
            agent: "claude".into(),
            model: None,
            worktree: None,
            transcript_path: None,
            pre_agent_snapshots: None,
            file_snapshots: None,
            edited_files: if edited.is_empty() {
                None
            } else {
                Some(edited.into_iter().map(std::string::ToString::to_string).collect())
            },
            read_files: if read.is_empty() {
                None
            } else {
                Some(read.into_iter().map(std::string::ToString::to_string).collect())
            },
            file_events: None,
            tool_usage: None,
            tool_failures: None,
            bash_commands: None,
            subagent_runs: None,
            thinking_duration_ms: None,
            compact_count: None,
            turn_count: None,
            context_tokens: None,
            context_window_size: None,
            current_turn_index: 0,
            current_turn_started_at: None,
            current_turn_hook_events: None,
            current_turn_tool_calls: None,
            last_turn_snapshot_id: None,
            pre_edit_pending: None,
            file_edit_chain: None,
            started_at: now,
            updated_at: now,
            ended_at: None,
        }
    }

    #[test]
    fn filter_by_recency_empty() {
        let result = filter_by_recency(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn tool_match_accepts_cursor_aliases() {
        assert!(is_agent_tool_match("agent", "composer"));
        assert!(is_agent_tool_match("cursor", "cursor"));
        assert!(is_agent_tool_match("claude", "claude"));
        assert!(!is_agent_tool_match("claude", "cursor"));
    }

    #[test]
    fn normalizes_absolute_paths_under_project_root() {
        let project_root = "/tmp/project";
        assert_eq!(
            normalize_path("/tmp/project/src/main.rs", project_root),
            "src/main.rs"
        );
        assert_eq!(normalize_path("src/lib.rs", project_root), "src/lib.rs");
        assert_eq!(
            normalize_path("/other/project/src/main.rs", project_root),
            "/other/project/src/main.rs"
        );
    }

    #[test]
    fn ms_to_secs_only_divides_millisecond_epoch_values() {
        assert_eq!(ms_to_secs(1_700_000_000_000), 1_700_000_000);
        assert_eq!(ms_to_secs(1_700_000_000), 1_700_000_000);
    }

    #[test]
    fn detect_file_interactions_refs_uses_active_session_data() {
        let sessions = [
            make_session("s1", vec!["a.rs"], vec![]),
            make_session("s2", vec!["a.rs"], vec![]),
        ];
        let refs: Vec<&_> = sessions.iter().collect();
        let (interactions, peers) = detect_file_interactions_refs(&refs, "/tmp");
        assert_eq!(interactions.len(), 1);
        assert_eq!(interactions[0].path, "a.rs");
        assert_eq!(peers.get("s1").unwrap(), &vec!["s2".to_string()]);
        assert_eq!(peers.get("s2").unwrap(), &vec!["s1".to_string()]);
    }

    #[test]
    fn detect_file_interactions_refs_no_overlap() {
        let sessions = [
            make_session("s1", vec!["a.rs"], vec![]),
            make_session("s2", vec!["b.rs"], vec![]),
        ];
        let refs: Vec<&_> = sessions.iter().collect();
        let (interactions, peers) = detect_file_interactions_refs(&refs, "/tmp");
        assert!(interactions.is_empty());
        assert!(peers.is_empty());
    }

    #[test]
    fn detect_file_interactions_refs_single_session() {
        let sessions = [make_session("s1", vec!["a.rs"], vec!["b.rs"])];
        let refs: Vec<&_> = sessions.iter().collect();
        let (interactions, _) = detect_file_interactions_refs(&refs, "/tmp");
        assert!(interactions.is_empty());
    }
}
