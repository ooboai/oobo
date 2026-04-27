use super::session_evidence::is_agent_tool_match;

/// A collected transcript with optional parent linkage for subagent sessions.
#[allow(dead_code)]
pub(in crate::git) struct CollectedTranscript {
    pub session_id: String,
    pub content: String,
    pub parent_session_id: Option<String>,
    pub subagent_type: Option<String>,
}

#[cfg(test)]
pub(in crate::git) fn build_sync_transcripts(
    transcripts: &[CollectedTranscript],
) -> (
    Vec<crate::remote::payload::TranscriptMessage>,
    Vec<crate::remote::payload::SessionTranscript>,
) {
    use crate::remote::payload;
    let mut structured = Vec::new();
    for ct in transcripts {
        let redacted = crate::redact::redact(&ct.content);
        let messages = parse_transcript_messages(&redacted);

        structured.push(payload::SessionTranscript {
            session_id: ct.session_id.clone(),
            parent_session_id: ct.parent_session_id.clone(),
            subagent_type: ct.subagent_type.clone(),
            messages,
        });
    }

    let flat = structured
        .iter()
        .flat_map(|st| st.messages.iter().cloned())
        .collect();

    (flat, structured)
}

/// Collect rich transcript content for active sessions (for full-transparency mode).
///
/// Priority order per session:
/// 1. Cursor's bubbleId: DB — includes thinking, tool calls, timestamps, tokens
/// 2. transcript_path from the stop hook payload
/// 3. Tool registry's find_transcript (JSONL/text file)
///
/// Also collects subagent transcripts from the `subagents/` directory
/// alongside the parent session's transcript.
pub(in crate::git) fn collect_session_transcripts(
    sessions: &[crate::hooks::state::ActiveSession],
    project_root: &str,
) -> Vec<CollectedTranscript> {
    let mut transcripts = Vec::new();

    for session in sessions {
        // 1. Collect the parent session's transcript.
        let mut found_parent = false;

        if is_cursor_session(&session.agent) {
            if let Some(rich) =
                crate::tools::cursor::composer_data::build_rich_transcript(&session.session_id)
            {
                transcripts.push(CollectedTranscript {
                    session_id: session.session_id.clone(),
                    content: rich,
                    parent_session_id: None,
                    subagent_type: None,
                });
                found_parent = true;
            }
        }

        if !found_parent {
            let raw = session
                .transcript_path
                .as_deref()
                .and_then(|tp| std::fs::read_to_string(tp).ok())
                .filter(|c| !c.is_empty());

            if let Some(content) = raw {
                transcripts.push(CollectedTranscript {
                    session_id: session.session_id.clone(),
                    content,
                    parent_session_id: None,
                    subagent_type: None,
                });
                found_parent = true;
            }
        }

        if !found_parent {
            let registry = crate::tools::registry();
            for tool in registry.all() {
                if is_agent_tool_match(&session.agent, tool.name()) {
                    if let Some(path) = tool.find_transcript(project_root, &session.session_id) {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            if !content.is_empty() {
                                transcripts.push(CollectedTranscript {
                                    session_id: session.session_id.clone(),
                                    content,
                                    parent_session_id: None,
                                    subagent_type: None,
                                });
                            }
                        }
                    }
                    break;
                }
            }
        }

        // 2. Always collect subagent transcripts for tools that support them.
        if is_cursor_session(&session.agent) {
            collect_cursor_subagent_transcripts(
                project_root,
                &session.session_id,
                &mut transcripts,
            );
        } else if is_claude_session(&session.agent) {
            collect_claude_subagent_transcripts(
                project_root,
                &session.session_id,
                &mut transcripts,
            );
        }
    }
    transcripts
}

#[cfg(test)]
fn parse_transcript_messages(text: &str) -> Vec<crate::remote::payload::TranscriptMessage> {
    use crate::remote::payload;
    // Detect Claude JSONL format: entries have both "type" and "message" top-level keys.
    // Checking for both prevents false positives from non-Claude transcripts.
    let is_claude_jsonl = text
        .lines()
        .take(5)
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .any(|v| {
            let ty = v.get("type").and_then(|t| t.as_str());
            matches!(ty, Some("user" | "assistant")) && v.get("message").is_some()
        });

    if is_claude_jsonl {
        return parse_claude_jsonl_transcript(text);
    }

    text.lines()
        .filter_map(|line| {
            let parsed: serde_json::Value = serde_json::from_str(line).ok()?;
            let role = parsed
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let text = parsed
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
                .or_else(|| parsed.get("text").and_then(|t| t.as_str()))
                .unwrap_or("")
                .to_string();

            let thinking = parsed
                .get("thinking")
                .and_then(|t| {
                    t.as_str()
                        .map(String::from)
                        .or_else(|| t.get("text").and_then(|v| v.as_str()).map(String::from))
                })
                .filter(|s| !s.is_empty());

            let timestamp_ms = parsed
                .get("timestamp")
                .and_then(|v| v.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis());

            if text.is_empty() && thinking.is_none() {
                return None;
            }
            Some(payload::TranscriptMessage {
                role,
                text: if text.is_empty() { None } else { Some(text) },
                thinking,
                tool_call: None,
                tool_result: None,
                timestamp_ms,
            })
        })
        .collect()
}

#[cfg(test)]
fn parse_claude_jsonl_transcript(text: &str) -> Vec<crate::remote::payload::TranscriptMessage> {
    crate::tools::claude::transcript::parse_rich_transcript_lines(text.lines())
}

/// Collect transcript files from the subagents/ directory for a Cursor session.
fn collect_cursor_subagent_transcripts(
    project_root: &str,
    parent_session_id: &str,
    transcripts: &mut Vec<CollectedTranscript>,
) {
    let subagents = crate::tools::cursor::transcript::find_subagent_transcripts(
        project_root,
        parent_session_id,
    );
    if subagents.is_empty() {
        return;
    }

    // Build subagent_type lookup from Cursor's session discovery.
    let type_map: std::collections::HashMap<String, String> =
        crate::tools::cursor::sessions_for_project(project_root)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| s.subagent_type.map(|t| (s.session_id, t)))
            .collect();

    for (subagent_id, path) in subagents {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if !content.is_empty() {
                let stype = type_map.get(&subagent_id).cloned();
                transcripts.push(CollectedTranscript {
                    session_id: subagent_id,
                    content,
                    parent_session_id: Some(parent_session_id.to_string()),
                    subagent_type: stype,
                });
            }
        }
    }
}

/// Collect transcript files from the subagents/ directory for a Claude Code session.
fn collect_claude_subagent_transcripts(
    project_root: &str,
    parent_session_id: &str,
    transcripts: &mut Vec<CollectedTranscript>,
) {
    let subagents = crate::tools::claude::transcript::find_subagent_transcripts(
        project_root,
        parent_session_id,
    );
    for (subagent_id, path) in subagents {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if !content.is_empty() {
                let stype = extract_claude_agent_id(&content);
                transcripts.push(CollectedTranscript {
                    session_id: subagent_id,
                    content,
                    parent_session_id: Some(parent_session_id.to_string()),
                    subagent_type: stype,
                });
            }
        }
    }
}

/// Extract `agentId` from the first entry of a Claude subagent JSONL.
fn extract_claude_agent_id(content: &str) -> Option<String> {
    for line in content.lines().take(5) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(id) = entry.get("agentId").and_then(|v| v.as_str()) {
                return Some(id.to_string());
            }
        }
    }
    None
}

fn is_cursor_session(agent: &str) -> bool {
    crate::core::tool::is_cursor_agent(agent)
}

fn is_claude_session(agent: &str) -> bool {
    agent == "claude"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_transcript_messages_extracts_thinking() {
        let lines = [
            r#"{"role":"assistant","text":"I see the issue.","thinking":{"text":"Let me analyze this carefully...","duration_ms":1500},"timestamp":"2026-01-15T10:00:01Z"}"#,
            r#"{"role":"assistant","text":"Here is my fix.","timestamp":"2026-01-15T10:00:05Z"}"#,
        ];
        let input = lines.join("\n");
        let msgs = parse_transcript_messages(&input);

        assert_eq!(msgs.len(), 2);
        assert_eq!(
            msgs[0].thinking.as_deref(),
            Some("Let me analyze this carefully...")
        );
        assert_eq!(msgs[0].text.as_deref(), Some("I see the issue."));
        assert!(msgs[0].timestamp_ms.is_some());

        assert!(msgs[1].thinking.is_none());
        assert_eq!(msgs[1].text.as_deref(), Some("Here is my fix."));
    }

    #[test]
    fn parse_transcript_messages_keeps_thinking_only_entries() {
        let input = r#"{"role":"assistant","thinking":{"text":"Internal reasoning step here."}}"#;
        let msgs = parse_transcript_messages(input);

        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].text.is_none());
        assert_eq!(
            msgs[0].thinking.as_deref(),
            Some("Internal reasoning step here.")
        );
    }

    #[test]
    fn parse_transcript_messages_accepts_plain_string_thinking() {
        let input = r#"{"role":"assistant","text":"result","thinking":"plain string thinking"}"#;
        let msgs = parse_transcript_messages(input);

        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].thinking.as_deref(), Some("plain string thinking"));
    }

    #[test]
    fn build_sync_transcripts_preserves_session_hierarchy() {
        let transcripts = [CollectedTranscript {
            session_id: "child".to_string(),
            content: r#"{"role":"assistant","text":"done"}"#.to_string(),
            parent_session_id: Some("parent".to_string()),
            subagent_type: Some("review".to_string()),
        }];

        let (flat, structured) = build_sync_transcripts(&transcripts);

        assert_eq!(flat.len(), 1);
        assert_eq!(structured.len(), 1);
        assert_eq!(structured[0].session_id, "child");
        assert_eq!(structured[0].parent_session_id.as_deref(), Some("parent"));
        assert_eq!(structured[0].subagent_type.as_deref(), Some("review"));
        assert_eq!(structured[0].messages[0].text.as_deref(), Some("done"));
    }

    #[test]
    fn extract_claude_agent_id_reads_first_jsonl_entries() {
        let content = r#"

{"type":"user","message":{"content":"start"}}
{"type":"assistant","agentId":"reviewer","message":{"content":"ok"}}
"#;

        assert_eq!(
            extract_claude_agent_id(content).as_deref(),
            Some("reviewer")
        );
    }

    #[test]
    fn collect_session_transcripts_uses_explicit_transcript_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let transcript_path = dir.path().join("session.jsonl");
        std::fs::write(&transcript_path, r#"{"role":"assistant","text":"hello"}"#).unwrap();

        let now = chrono::Utc::now().timestamp();
        let session = crate::hooks::state::ActiveSession {
            session_id: "s1".to_string(),
            agent: "claude".to_string(),
            model: None,
            worktree: None,
            transcript_path: Some(transcript_path.to_string_lossy().to_string()),
            pre_agent_snapshots: None,
            file_snapshots: None,
            edited_files: None,
            read_files: None,
            tool_usage: None,
            tool_failures: None,
            bash_commands: None,
            subagent_runs: None,
            thinking_duration_ms: None,
            compact_count: None,
            current_turn_index: 0,
            current_turn_started_at: None,
            current_turn_hook_events: None,
            current_turn_tool_calls: None,
            last_turn_snapshot_id: None,
            started_at: now,
            updated_at: now,
        };

        let transcripts = collect_session_transcripts(&[session], dir.path().to_str().unwrap());

        assert_eq!(transcripts.len(), 1);
        assert_eq!(transcripts[0].session_id, "s1");
        assert_eq!(
            transcripts[0].content,
            r#"{"role":"assistant","text":"hello"}"#
        );
        assert!(transcripts[0].parent_session_id.is_none());
        assert!(transcripts[0].subagent_type.is_none());
    }
}
