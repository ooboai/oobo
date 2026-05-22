use crate::core::anchor::{FileInteraction, LinkType, SessionLink};
use crate::hooks;
use crate::redact;

use super::session_evidence::{
    detect_file_interactions_refs, filter_by_recency, is_agent_tool_match, ms_to_secs,
    normalize_path,
};

pub(super) struct SessionResolution {
    pub(super) session_links: Vec<SessionLink>,
    pub(super) file_interactions: Vec<FileInteraction>,
}

pub(super) fn resolve_sessions(
    project_root: &str,
    active_sessions: &[hooks::state::ActiveSession],
    ai_files_touched: &[(String, String)],
    now_epoch: i64,
) -> SessionResolution {
    let cursor_ids: Vec<String> = active_sessions
        .iter()
        .filter(|s| is_cursor_session(&s.agent))
        .map(|s| s.session_id.clone())
        .collect();
    let bubble_data = crate::tools::cursor::composer_data::preload_bubble_data_for(&cursor_ids);
    let composer_data = crate::tools::cursor::composer_data::preload_composer_data_for(&cursor_ids);

    let mut session_links: Vec<SessionLink> = active_sessions
        .iter()
        .map(|s| {
            let touched: Vec<String> = ai_files_touched
                .iter()
                .filter(|(_, agent)| agent == &s.agent)
                .map(|(path, _)| path.clone())
                .collect();

            let native = extract_live_stats(
                &s.session_id,
                &s.agent,
                project_root,
                &bubble_data,
                &composer_data,
            );

            let native_inp = native.as_ref().and_then(|n| n.input_tokens);
            let native_out = native.as_ref().and_then(|n| n.output_tokens);
            let has_native_tokens =
                native_inp.is_some_and(|v| v > 0) || native_out.is_some_and(|v| v > 0);

            let (final_input, final_output, is_estimated) = if has_native_tokens {
                (native_inp, native_out, false)
            } else {
                let msgs = load_session_messages(
                    &s.session_id,
                    &s.agent,
                    project_root,
                    &bubble_data,
                    &composer_data,
                );
                if let Some((inp, out)) = count_tokens_from_messages(&msgs, s.model.as_deref()) {
                    (Some(inp), Some(out), true)
                } else {
                    (None, None, false)
                }
            };

            let duration_fallback = {
                let elapsed = now_epoch - s.started_at;
                if elapsed > 0 {
                    Some(elapsed as u64)
                } else {
                    None
                }
            };

            let mut link = SessionLink {
                session_id: s.session_id.clone(),
                agent: s.agent.clone(),
                model: s.model.clone(),
                link_type: LinkType::Explicit,
                input_tokens: final_input,
                output_tokens: final_output,
                cache_read_tokens: native.as_ref().and_then(|n| n.cache_read_tokens),
                cache_creation_tokens: native.as_ref().and_then(|n| n.cache_creation_tokens),
                duration_secs: native
                    .as_ref()
                    .and_then(|n| n.duration_secs)
                    .or(duration_fallback),
                tool_calls: native
                    .as_ref()
                    .map(|n| n.tool_call_count)
                    .filter(|&c| c > 0),
                files_touched: if touched.is_empty() {
                    None
                } else {
                    Some(
                        touched
                            .into_iter()
                            .map(|p| redact::sanitize_path(&p, project_root))
                            .collect(),
                    )
                },
                tool_usage: s.tool_usage.clone(),
                tool_failures: s.tool_failures,
                subagent_count: s
                    .subagent_runs
                    .as_ref()
                    .map(|r| r.len() as u32)
                    .filter(|&c| c > 0),
                bash_commands: s.bash_commands.as_ref().map(|cmds| {
                    cmds.iter()
                        .map(|c| redact::sanitize_for_public(c, project_root))
                        .collect()
                }),
                thinking_duration_ms: s.thinking_duration_ms,
                compact_count: s.compact_count,
                is_subagent: false,
                parent_session_id: None,
                subagent_type: None,
                is_estimated,
                peer_session_ids: Vec::new(),
                context_tokens: s.context_tokens,
                context_window_size: s.context_window_size,
            };

            if link.model.is_none() && is_cursor_session(&s.agent) {
                if let Some(bubble) = bubble_data.get(&s.session_id) {
                    if bubble.model.is_some() {
                        link.model.clone_from(&bubble.model);
                    }
                }
                if link.model.is_none() {
                    if let Some(composer) = composer_data.get(&s.session_id) {
                        if composer.model.is_some() {
                            link.model.clone_from(&composer.model);
                        }
                    }
                }
            }

            link
        })
        .collect();

    // Filter out subagent sessions before detecting file interactions:
    // a parent and its subagent touching the same files is expected, not novel.
    let subagent_ids: std::collections::HashSet<String> = active_sessions
        .iter()
        .filter_map(|s| s.subagent_runs.as_ref())
        .flat_map(|runs| runs.iter().map(|r| r.agent_id.clone()))
        .collect();
    let top_level_sessions: Vec<&_> = active_sessions
        .iter()
        .filter(|s| !subagent_ids.contains(&s.session_id))
        .collect();
    let (raw_file_interactions, peer_map) =
        detect_file_interactions_refs(&top_level_sessions, project_root);
    let file_interactions: Vec<_> = raw_file_interactions
        .into_iter()
        .map(|mut fi| {
            fi.path = redact::sanitize_path(&fi.path, project_root);
            fi
        })
        .collect();
    for link in &mut session_links {
        if let Some(peers) = peer_map.get(&link.session_id) {
            link.peer_session_ids = peers.clone();
        }
    }

    SessionResolution {
        session_links,
        file_interactions,
    }
}

/// Collect files touched by AI tools during active sessions.
/// Returns (file_path, agent_name) pairs.
///
/// For Cursor sessions, reads `edit_file_v2` tool calls from the bubble DB
/// which gives us exact file paths. Falls back to the tool registry for
/// non-Cursor tools.
pub(super) fn collect_ai_files_touched(
    cfg: &crate::config::Config,
    project_root: &str,
    active_sessions: &[hooks::state::ActiveSession],
    since_epoch: i64,
) -> Vec<(String, String)> {
    if active_sessions.is_empty() {
        return Vec::new();
    }

    let mut result: Vec<(String, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for session in active_sessions {
        if let Some(ref snapshots) = session.file_snapshots {
            for file in snapshots.keys() {
                let normalized = normalize_path(file, project_root);
                if seen.insert(normalized.clone()) {
                    result.push((normalized, session.agent.clone()));
                }
            }
        }

        if is_cursor_session(&session.agent) {
            let files = crate::tools::cursor::composer_data::files_edited_in_session(
                &session.session_id,
                project_root,
                since_epoch,
            );
            for file in files {
                if seen.insert(file.clone()) {
                    result.push((file, session.agent.clone()));
                }
            }
            continue;
        }

        let registry = crate::tools::registry();
        for tool in registry.all() {
            if !is_agent_tool_match(&session.agent, tool.name()) {
                continue;
            }

            if !tool.enabled(cfg) {
                break;
            }

            if let Ok(sessions) = tool.sessions_for_project(project_root) {
                if let Some(tool_session) =
                    sessions.iter().find(|s| s.session_id == session.session_id)
                {
                    if let Some(stats) = tool.extract_native_stats(tool_session) {
                        for file in &stats.files_touched {
                            let normalized = normalize_path(file, project_root);
                            if seen.insert(normalized.clone()) {
                                result.push((normalized, session.agent.clone()));
                            }
                        }
                    }
                }
            }

            break;
        }
    }

    result
}

/// Filter active sessions to only those relevant to the current commit.
///
/// Primary signal: **file overlap** — if a session edited files that appear in
/// the commit, it's linked. This is much more reliable than timestamps because
/// we know exactly which files each Cursor session touched via `edit_file_v2`
/// bubbles.
///
/// Fallback (non-Cursor tools or when DB is unavailable): recency-based
/// heuristic — most recently updated session within a reasonable window.
///
/// When multiple sessions touch the same file, all are included (they both
/// contributed).
/// `since_epoch`: only count edits after this timestamp (parent commit time).
pub(super) fn filter_relevant_sessions(
    sessions: &[crate::hooks::state::ActiveSession],
    project_root: &str,
    committed_files: &[String],
    since_epoch: i64,
) -> Vec<crate::hooks::state::ActiveSession> {
    if sessions.is_empty() {
        return Vec::new();
    }

    let committed_set: std::collections::HashSet<&str> = committed_files
        .iter()
        .map(std::string::String::as_str)
        .collect();

    let mut matched_by_files: Vec<crate::hooks::state::ActiveSession> = Vec::new();
    let mut unresolved: Vec<&crate::hooks::state::ActiveSession> = Vec::new();

    for session in sessions {
        if is_cursor_session(&session.agent) {
            let edited = crate::tools::cursor::composer_data::files_edited_in_session(
                &session.session_id,
                project_root,
                since_epoch,
            );
            let has_overlap = edited.iter().any(|f| committed_set.contains(f.as_str()));
            if has_overlap {
                matched_by_files.push(session.clone());
            }
            // If a Cursor session edited files but none overlap, skip it —
            // it was working on something else.
            // If it edited zero files (e.g. DB unavailable), fall through
            // to recency.
            if edited.is_empty() && !has_overlap {
                unresolved.push(session);
            }
        } else {
            unresolved.push(session);
        }
    }

    if !matched_by_files.is_empty() {
        let now = chrono::Utc::now().timestamp();
        for s in &unresolved {
            if now - s.updated_at < 120 {
                matched_by_files.push((*s).clone());
            }
        }
        return matched_by_files;
    }

    // No file-based matches — fall back to recency heuristic, but only for
    // sessions we couldn't resolve (unknown files, non-Cursor tools, DB
    // unavailable). Sessions that were explicitly rejected because they edited
    // files not in this commit are NOT reconsidered.
    let unresolved_owned: Vec<crate::hooks::state::ActiveSession> =
        unresolved.into_iter().cloned().collect();
    filter_by_recency(&unresolved_owned)
}

/// Fallback session discovery: scan each enabled tool's native storage for
/// recent sessions in this project. Used when the DB has no matching active
/// sessions (e.g. hooks fired before `git init`, or hooks weren't installed
/// for this project).
///
/// Only returns sessions updated after `since_epoch` (the parent commit time)
/// and within the last 2 hours — we don't want to link stale sessions.
pub(super) fn discover_sessions_from_tools(
    cfg: &crate::config::Config,
    project_root: &str,
    since_epoch: i64,
) -> Vec<crate::hooks::state::ActiveSession> {
    let now_secs = chrono::Utc::now().timestamp();
    let max_age_secs: i64 = 7200; // 2 hours
    let cutoff_secs = std::cmp::max(since_epoch, now_secs - max_age_secs);

    let registry = crate::tools::registry();
    let mut discovered: Vec<crate::hooks::state::ActiveSession> = Vec::new();
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    for tool in registry.enabled(cfg) {
        let sessions = match tool.sessions_for_project(project_root) {
            Ok(s) => s,
            Err(_) => continue,
        };

        for session in sessions {
            if session.is_subagent() {
                continue;
            }

            let updated_ms = session.updated_at.or(session.created_at).unwrap_or(0);
            // Tool sessions store timestamps in milliseconds; normalize to
            // seconds to match hook-based ActiveSession conventions.
            let updated_secs = ms_to_secs(updated_ms);
            if updated_secs <= cutoff_secs {
                continue;
            }
            if seen_ids.contains(&session.session_id) {
                continue;
            }
            seen_ids.insert(session.session_id.clone());

            let created_secs = session.created_at.map_or(updated_secs, ms_to_secs);

            discovered.push(crate::hooks::state::ActiveSession {
                session_id: session.session_id,
                agent: session.source.clone(),
                model: None,
                worktree: Some(project_root.to_string()),
                transcript_path: None,
                pre_agent_snapshots: None,
                file_snapshots: None,
                edited_files: None,
                read_files: None,
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
                started_at: created_secs,
                updated_at: updated_secs,
                ended_at: None,
            });
        }
    }

    discovered
}

fn is_cursor_session(agent: &str) -> bool {
    crate::core::tool::is_cursor_agent(agent)
}

/// Extract live session stats directly from the tool's data files at commit
/// time. No prior manual reindex needed — reads Cursor's state.vscdb, Claude's
/// JSONL transcripts, etc.
fn extract_live_stats(
    session_id: &str,
    agent: &str,
    project_root: &str,
    bubble_data: &std::collections::HashMap<
        String,
        crate::tools::cursor::composer_data::BubbleSession,
    >,
    preloaded_composer: &std::collections::HashMap<
        String,
        crate::tools::cursor::composer_data::ComposerSession,
    >,
) -> Option<crate::analytics::NativeStats> {
    use crate::tools::cursor::composer_data;

    if is_cursor_session(agent) {
        if let Some(bubble) = bubble_data.get(session_id) {
            let mut stats = composer_data::native_stats_from_bubble(bubble);
            if stats.duration_secs.is_none() {
                if let Some(cs) = preloaded_composer.get(session_id) {
                    if let (Some(c), Some(u)) = (cs.created_at, cs.last_updated_at) {
                        if u > c {
                            stats.duration_secs = Some(((u - c) / 1000) as u64);
                        }
                    }
                }
            }
            return Some(stats);
        }
        if let Some(cs) = preloaded_composer.get(session_id) {
            return Some(composer_data::native_stats_from_session(cs));
        }
        return None;
    }

    if agent == "claude" {
        return crate::tools::claude::transcript::extract_native_stats(project_root, session_id);
    }

    if agent == "gemini" {
        if let Some(stats) =
            crate::tools::gemini::transcript::stats_for_session(project_root, session_id)
        {
            return Some(crate::analytics::NativeStats {
                input_tokens: stats.input_tokens,
                output_tokens: stats.output_tokens,
                cache_read_tokens: stats.cache_read_tokens,
                cache_creation_tokens: stats.cache_creation_tokens,
                duration_secs: stats.duration_secs,
                files_touched: stats.files_touched,
                tool_call_count: stats.tool_call_count,
            });
        }
        return None;
    }

    // Fall through to the tool registry for all other tools (codex, opencode,
    // aider, copilot, windsurf, trae, zed) so commit-time stats aren't lost.
    let registry = crate::tools::registry();
    for tool in registry.all() {
        if !is_agent_tool_match(agent, tool.name()) {
            continue;
        }
        if let Ok(sessions) = tool.sessions_for_project(project_root) {
            if let Some(tool_session) = sessions.iter().find(|s| s.session_id == session_id) {
                return tool.extract_native_stats(tool_session);
            }
        }
        break;
    }

    None
}

/// Count tokens from a message slice using tiktoken.
fn count_tokens_from_messages(
    messages: &[crate::core::message::Message],
    model: Option<&str>,
) -> Option<(u64, u64)> {
    use crate::analytics::tokenizer;

    if messages.is_empty() {
        return None;
    }

    let family = model.map_or(tokenizer::ModelFamily::Cl100k, tokenizer::detect_family);
    let inp: u64 = messages
        .iter()
        .filter(|m| tokenizer::is_input_role(&m.role))
        .map(|m| tokenizer::count_tokens(&m.text, family))
        .sum();
    let out: u64 = messages
        .iter()
        .filter(|m| tokenizer::is_output_role(&m.role))
        .map(|m| tokenizer::count_tokens(&m.text, family))
        .sum();
    if inp > 0 || out > 0 {
        Some((inp, out))
    } else {
        None
    }
}

/// Load conversation messages for a session, preferring already-loaded data.
fn load_session_messages(
    session_id: &str,
    agent: &str,
    project_root: &str,
    bubble_data: &std::collections::HashMap<
        String,
        crate::tools::cursor::composer_data::BubbleSession,
    >,
    preloaded_composer: &std::collections::HashMap<
        String,
        crate::tools::cursor::composer_data::ComposerSession,
    >,
) -> Vec<crate::core::message::Message> {
    if is_cursor_session(agent) {
        if let Some(bs) = bubble_data.get(session_id) {
            if !bs.messages.is_empty() {
                return bs.messages.clone();
            }
        }
        if let Some(cs) = preloaded_composer.get(session_id) {
            if !cs.messages.is_empty() {
                return cs.messages.clone();
            }
        }
        Vec::new()
    } else {
        let source = crate::core::tool::normalize_source(agent);
        crate::session::parse_messages_for_session(project_root, session_id, source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_tokens_from_messages_with_bubble_data() {
        use crate::core::message::Message;
        use crate::tools::cursor::composer_data::BubbleSession;

        let mut bubble_data = std::collections::HashMap::new();
        bubble_data.insert(
            "test-session".to_string(),
            BubbleSession {
                messages: vec![
                    Message {
                        role: "user".to_string(),
                        text: "How do I implement a binary search in Rust?".to_string(),
                        timestamp_ms: Some(1000),
                    },
                    Message {
                        role: "assistant".to_string(),
                        text: "Here is a binary search implementation in Rust using iterators and pattern matching for clean, idiomatic code.".to_string(),
                        timestamp_ms: Some(2000),
                    },
                ],
                total_input_tokens: 0,
                total_output_tokens: 0,
                files_touched: Vec::new(),
                code_block_count: 0,
                tool_call_count: 0,
                is_agentic: true,
                model: None,
            },
        );

        let composer_data = std::collections::HashMap::new();
        let msgs = load_session_messages(
            "test-session",
            "cursor",
            "/tmp/project",
            &bubble_data,
            &composer_data,
        );
        let result = count_tokens_from_messages(&msgs, None);
        assert!(result.is_some(), "should produce tiktoken estimates");
        let (inp, out) = result.unwrap();
        assert!(inp > 0, "input tokens should be > 0");
        assert!(out > 0, "output tokens should be > 0");
    }

    #[test]
    fn test_count_tokens_from_messages_with_model() {
        use crate::core::message::Message;

        let messages = vec![
            Message {
                role: "user".to_string(),
                text: "What is Rust programming language?".to_string(),
                timestamp_ms: None,
            },
            Message {
                role: "assistant".to_string(),
                text: "Rust is a systems programming language focused on safety and performance."
                    .to_string(),
                timestamp_ms: None,
            },
        ];

        let result_cl100k = count_tokens_from_messages(&messages, Some("claude-sonnet-4"));
        let result_o200k = count_tokens_from_messages(&messages, Some("gpt-4o"));
        assert!(result_cl100k.is_some());
        assert!(result_o200k.is_some());
        let (inp1, out1) = result_cl100k.unwrap();
        let (inp2, out2) = result_o200k.unwrap();
        assert!(inp1 > 0 && out1 > 0);
        assert!(inp2 > 0 && out2 > 0);
    }

    #[test]
    fn test_load_session_messages_empty_bubble() {
        let mut bubble_data = std::collections::HashMap::new();
        bubble_data.insert(
            "empty-session".to_string(),
            crate::tools::cursor::composer_data::BubbleSession::default(),
        );
        let composer_data = std::collections::HashMap::new();

        let msgs = load_session_messages(
            "empty-session",
            "cursor",
            "/tmp/project",
            &bubble_data,
            &composer_data,
        );
        let result = count_tokens_from_messages(&msgs, None);
        assert!(result.is_none(), "empty messages should return None");
    }

    #[test]
    fn test_load_session_messages_unknown_session() {
        let bubble_data = std::collections::HashMap::new();
        let composer_data = std::collections::HashMap::new();
        let msgs = load_session_messages(
            "nonexistent-session",
            "cursor",
            "/tmp/project",
            &bubble_data,
            &composer_data,
        );
        let result = count_tokens_from_messages(&msgs, None);
        assert!(result.is_none(), "unknown session should return None");
    }

    #[test]
    fn test_load_session_messages_falls_back_to_composer_data() {
        use crate::core::message::Message;
        use crate::tools::cursor::composer_data::ComposerSession;

        let bubble_data = std::collections::HashMap::new();
        let mut composer_data = std::collections::HashMap::new();
        composer_data.insert(
            "composer-session".to_string(),
            ComposerSession {
                composer_id: "composer-session".to_string(),
                name: "test".to_string(),
                mode: "agent".to_string(),
                status: "done".to_string(),
                created_at: Some(1000),
                last_updated_at: Some(2000),
                messages: vec![
                    Message {
                        role: "user".to_string(),
                        text: "Help me write a function".to_string(),
                        timestamp_ms: Some(1000),
                    },
                    Message {
                        role: "assistant".to_string(),
                        text: "Here is a function implementation.".to_string(),
                        timestamp_ms: Some(1500),
                    },
                ],
                files_touched: Vec::new(),
                code_block_count: 0,
                is_agentic: true,
                model: None,
            },
        );

        let msgs = load_session_messages(
            "composer-session",
            "cursor",
            "/tmp/project",
            &bubble_data,
            &composer_data,
        );
        assert!(!msgs.is_empty(), "should load messages from composer data");
        let result = count_tokens_from_messages(&msgs, None);
        assert!(result.is_some());
    }

    #[test]
    fn test_count_tokens_includes_system_and_tool_roles() {
        use crate::core::message::Message;

        let messages = vec![
            Message {
                role: "system".to_string(),
                text: "You are a helpful coding assistant with access to tools.".to_string(),
                timestamp_ms: None,
            },
            Message {
                role: "user".to_string(),
                text: "Read the file".to_string(),
                timestamp_ms: None,
            },
            Message {
                role: "assistant".to_string(),
                text: "Reading the file now.".to_string(),
                timestamp_ms: None,
            },
            Message {
                role: "tool".to_string(),
                text: "fn main() { println!(\"hello world\"); }".to_string(),
                timestamp_ms: None,
            },
        ];

        let result = count_tokens_from_messages(&messages, None);
        assert!(result.is_some());
        let (inp, _out) = result.unwrap();
        let user_only_msgs = vec![Message {
            role: "user".to_string(),
            text: "Read the file".to_string(),
            timestamp_ms: None,
        }];
        let user_only = count_tokens_from_messages(&user_only_msgs, None);
        let (user_inp, _) = user_only.unwrap();
        assert!(
            inp > user_inp,
            "input tokens should include system + tool, not just user"
        );
    }

    fn make_session(id: &str, agent: &str) -> crate::hooks::state::ActiveSession {
        let now = chrono::Utc::now().timestamp();
        crate::hooks::state::ActiveSession {
            session_id: id.to_string(),
            agent: agent.to_string(),
            model: None,
            worktree: None,
            transcript_path: None,
            pre_agent_snapshots: None,
            file_snapshots: None,
            edited_files: None,
            read_files: None,
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
    fn test_filter_ended_session_included_via_recency() {
        let mut session = make_session("ended-1", "claude");
        session.ended_at = Some(chrono::Utc::now().timestamp());
        session.updated_at = chrono::Utc::now().timestamp();

        let committed = vec!["src/main.rs".to_string()];
        let result = filter_relevant_sessions(&[session], "/tmp/fake", &committed, 0);
        assert_eq!(
            result.len(),
            1,
            "ended session should be included via recency (same as active sessions)"
        );
        assert_eq!(result[0].session_id, "ended-1");
    }

    #[test]
    fn test_filter_active_session_uses_recency() {
        let mut session = make_session("active-1", "claude");
        session.updated_at = chrono::Utc::now().timestamp();

        let committed = vec!["src/main.rs".to_string()];
        let result = filter_relevant_sessions(&[session], "/tmp/fake", &committed, 0);
        assert_eq!(
            result.len(),
            1,
            "active session should be included via recency fallback"
        );
    }

    #[test]
    fn test_filter_stale_ended_session_excluded() {
        let mut session = make_session("stale-1", "claude");
        session.ended_at = Some(chrono::Utc::now().timestamp() - 7200);
        session.updated_at = chrono::Utc::now().timestamp() - 7200;

        let committed = vec!["src/main.rs".to_string()];
        let result = filter_relevant_sessions(&[session], "/tmp/fake", &committed, 0);
        assert!(
            result.is_empty(),
            "stale ended session should be excluded by recency filter"
        );
    }
}
