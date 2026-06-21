pub mod install;
pub mod routing;
pub mod state;
pub mod store;

use serde::Deserialize;

/// Lifecycle event received from an agent tool via `oobo hooks agent <event>`.
#[derive(Debug, Deserialize)]
pub struct HookEvent {
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub workspace_roots: Vec<String>,
    #[serde(default)]
    pub loop_count: Option<u32>,
    #[serde(default)]
    pub context_tokens: Option<u64>,
    #[serde(default)]
    pub context_window_size: Option<u64>,
    /// Captures unknown fields from hook payloads for forward compatibility.
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// Handle a lifecycle event from an agent tool.
///
/// Called by `oobo hooks agent <event> [--tool <name>]` which receives JSON on stdin.
/// The `--tool` flag provides explicit tool identity (preferred over payload guessing).
/// This is internal plumbing  --  never typed by the user.
pub fn handle_event(
    event_name: &str,
    payload: &str,
    tool_flag: Option<&str>,
) -> crate::error::Result<()> {
    let mut event: HookEvent = match serde_json::from_str(payload) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(event = event_name, %e, "malformed hook payload");
            return Ok(());
        }
    };

    if event.event.is_empty() {
        event.event = event_name.to_string();
    }

    let cwd = event
        .workspace_roots
        .first()
        .cloned()
        .or(event.cwd.clone())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        });

    let project_root = crate::git::proxy::project_root_from(&cwd);

    let agent = tool_flag.unwrap_or_else(|| {
        event
            .agent
            .as_deref()
            .or(event.extra.get("composer_mode").and_then(|v| v.as_str()))
            .unwrap_or("unknown")
    });

    let session_id_field = event
        .session_id
        .as_deref()
        .or(event.extra.get("conversation_id").and_then(|v| v.as_str()))
        .filter(|s| !s.is_empty());

    match event_name {
        "session-start" => {
            let session_id = session_id_field.ok_or(crate::error::OoboError::Config(
                "session-start requires session_id".into(),
            ))?;
            state::write_session(&project_root, session_id, agent, event.model.as_deref())?;
            // Explicit resume lineage only — tools that report the prior
            // session id at session-start get a lineage link; we never
            // infer one (a wrong link is worse than none).
            let resumed_from = event
                .extra
                .get("resumed_from")
                .or_else(|| event.extra.get("previous_session_id"))
                .or_else(|| event.extra.get("source_session_id"))
                .and_then(|v| v.as_str())
                .filter(|prior| !prior.is_empty() && *prior != session_id);
            if let Some(prior) = resumed_from {
                let _ = state::record_resumed_from(&project_root, session_id, prior);
            }
            if !project_root.is_empty() {
                // Registry note (pointer resolution) + self-heal of any
                // missing git hooks for this repo — a session that fires
                // this event proves the tool side works; the repo side
                // must match.
                crate::project::registry_note(&project_root);
                self_heal_project_hooks(&project_root);
            }
            // For non-Cursor agents, capture pre-agent state at session start.
            // Cursor uses the more precise `before-submit-prompt` hook instead.
            if !project_root.is_empty() && !is_cursor_agent(agent) {
                let _ = state::snapshot_pre_agent_state(&project_root, session_id);
            }
        }
        "session-end" => {
            if let Some(sid) = session_id_field {
                state::remove_session(&project_root, sid);
            }
        }
        "before-submit-prompt" => {
            if let Some(sid) = session_id_field {
                if let Err(e) =
                    state::ensure_session(&project_root, sid, agent, event.model.as_deref())
                {
                    tracing::warn!(%e, session = sid, "ensure_session failed (lock contention?)");
                }
                if let Err(e) = state::start_turn(&project_root, sid) {
                    tracing::warn!(%e, session = sid, "start_turn failed (lock contention?)");
                }
                if let Err(e) = state::record_hook_event(
                    &project_root,
                    sid,
                    event_name,
                    Some(event.extra.clone()),
                ) {
                    tracing::warn!(%e, session = sid, "record_hook_event failed");
                }
                if !project_root.is_empty() {
                    if let Err(e) = state::snapshot_pre_agent_state(&project_root, sid) {
                        tracing::warn!(%e, session = sid, "snapshot_pre_agent_state failed");
                    }
                }
            }
        }
        "pre-tool-use" => {
            if let Some(sid) = session_id_field {
                let _ = state::ensure_session(&project_root, sid, agent, event.model.as_deref());
                let tool_name = event
                    .extra
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let tool_input = event.extra.get("tool_input");

                if is_file_mutating_tool(tool_name) || tool_name.is_empty() {
                    let file_path = event
                        .extra
                        .get("file_path")
                        .and_then(|v| v.as_str())
                        .or_else(|| {
                            tool_input
                                .and_then(|ti| ti.get("file_path").or_else(|| ti.get("path")))
                                .and_then(|v| v.as_str())
                        });

                    // Route by the edited file's repo, not the session's
                    // cwd  --  a session in project X can edit project Y.
                    if let Some(routed) =
                        file_path.and_then(|p| routing::route_file(p, &project_root))
                    {
                        if routed.foreign {
                            let _ = state::snapshot_pre_edit_file_in_repo(
                                &project_root,
                                sid,
                                &routed.repo_root,
                                &routed.rel,
                            );
                        } else {
                            let _ = state::snapshot_pre_edit_file(&project_root, sid, &routed.rel);
                        }
                    }
                }
            }
        }
        "after-tool-use" | "after-file-edit" => {
            if let Some(sid) = session_id_field {
                let mut batch =
                    state::SessionBatch::open(&project_root, sid, agent, event.model.as_deref())?;

                let tool_name = event
                    .extra
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let tool_input = event.extra.get("tool_input");

                {
                    let s = batch.state_mut();

                    if s.current_turn_started_at.is_none() {
                        s.current_turn_started_at = Some(chrono::Utc::now().timestamp());
                    }
                    let mut events = s.current_turn_hook_events.take().unwrap_or_default();
                    events.push(crate::core::turn::TurnHookEvent {
                        name: event_name.to_string(),
                        observed_at: chrono::Utc::now().timestamp(),
                        payload: Some(event.extra.clone()),
                    });
                    s.current_turn_hook_events = Some(events);

                    if !project_root.is_empty() && !tool_name.is_empty() {
                        let input_summary = summarize_tool_input_hook(tool_name, tool_input);

                        let mut usage = s.tool_usage.take().unwrap_or_default();
                        *usage.entry(tool_name.to_string()).or_insert(0) += 1;
                        s.tool_usage = Some(usage);
                        if matches!(tool_name, "Bash" | "Shell") {
                            if let Some(cmd) = &input_summary {
                                let mut cmds = s.bash_commands.take().unwrap_or_default();
                                const MAX_BASH: usize = 50;
                                if cmds.len() >= MAX_BASH {
                                    cmds.drain(..=cmds.len() - MAX_BASH);
                                }
                                cmds.push(cmd.clone());
                                s.bash_commands = Some(cmds);
                            }
                        }

                        let mut calls = s.current_turn_tool_calls.take().unwrap_or_default();
                        calls.push(crate::core::turn::TurnToolCall {
                            name: tool_name.to_string(),
                            observed_at: chrono::Utc::now().timestamp(),
                            input: tool_input.cloned(),
                            output: event.extra.get("tool_output").cloned(),
                            failed: false,
                        });
                        s.current_turn_tool_calls = Some(calls);
                    }
                }

                // Route the mutated file by its own repo  --  edits to a
                // foreign repo land in that repo's capture bucket so its
                // provenance is self-contained.
                let mutating_path = if is_file_mutating_tool(tool_name) || tool_name.is_empty() {
                    event
                        .extra
                        .get("file_path")
                        .and_then(|v| v.as_str())
                        .or_else(|| {
                            tool_input
                                .and_then(|ti| ti.get("file_path").or_else(|| ti.get("path")))
                                .and_then(|v| v.as_str())
                        })
                } else {
                    None
                };
                let mutating_route =
                    mutating_path.and_then(|abs| routing::route_file(abs, &project_root));

                // A mutation outside ANY git repo can't be attributed to a
                // store, but it must never vanish silently — record it in
                // the global no-repo ledger so the audit trail is complete.
                if let (Some(path), None) = (mutating_path, &mutating_route) {
                    record_no_repo_touch(sid, agent, tool_name, path);
                }

                if let Some(ref routed) = mutating_route {
                    let now = chrono::Utc::now().timestamp();
                    let s = batch.state_mut();
                    if routed.foreign {
                        let capture = s.foreign_capture_mut(&routed.repo_root);
                        if capture.turn_started_at.is_none() {
                            capture.turn_started_at = Some(now);
                        }
                        capture
                            .edited_files
                            .get_or_insert_with(Default::default)
                            .insert(routed.rel.clone());
                        capture
                            .turn_files
                            .get_or_insert_with(Default::default)
                            .insert(routed.rel.clone());
                        capture
                            .file_events
                            .get_or_insert_with(Default::default)
                            .push(state::FileEvent {
                                path: routed.rel.clone(),
                                action: "write".into(),
                                timestamp: now,
                            });
                    } else {
                        let mut files = s.edited_files.take().unwrap_or_default();
                        files.insert(routed.rel.clone());
                        s.edited_files = Some(files);
                        let mut fe = s.file_events.take().unwrap_or_default();
                        fe.push(state::FileEvent {
                            path: routed.rel.clone(),
                            action: "write".into(),
                            timestamp: now,
                        });
                        s.file_events = Some(fe);
                    }
                }

                if !project_root.is_empty() && is_read_tool(tool_name) {
                    let file_path = event
                        .extra
                        .get("file_path")
                        .and_then(|v| v.as_str())
                        .or_else(|| {
                            tool_input
                                .and_then(|ti| ti.get("file_path"))
                                .and_then(|v| v.as_str())
                        })
                        .or_else(|| {
                            if is_dir_scoped_tool(tool_name) {
                                None
                            } else {
                                tool_input
                                    .and_then(|ti| ti.get("path"))
                                    .and_then(|v| v.as_str())
                            }
                        });

                    if let Some(abs_path) = file_path {
                        if !abs_path.ends_with('/') && abs_path != "." {
                            let rel = make_relative(abs_path, &project_root);
                            if !rel.starts_with('/') && !rel.starts_with("..") {
                                let s = batch.state_mut();
                                let mut files = s.read_files.take().unwrap_or_default();
                                files.insert(rel.clone());
                                s.read_files = Some(files);
                                let mut fe = s.file_events.take().unwrap_or_default();
                                fe.push(state::FileEvent {
                                    path: rel,
                                    action: "read".into(),
                                    timestamp: chrono::Utc::now().timestamp(),
                                });
                                s.file_events = Some(fe);
                            }
                        }
                    }
                }

                batch.flush()?;

                // record_post_edit_file does git hash-object (subprocess) +
                // its own mutate round-trip  --  must happen after flush.
                if let Some(ref routed) = mutating_route {
                    let tool = if tool_name.is_empty() {
                        None
                    } else {
                        Some(tool_name)
                    };
                    if routed.foreign {
                        if let Err(e) = state::record_post_edit_file_in_repo(
                            &project_root,
                            sid,
                            &routed.repo_root,
                            &routed.rel,
                            tool,
                        ) {
                            tracing::warn!(%e, session = sid, file = %routed.rel, "record_post_edit_file_in_repo failed");
                        }
                    } else if let Err(e) =
                        state::record_post_edit_file(&project_root, sid, &routed.rel, tool)
                    {
                        tracing::warn!(%e, session = sid, file = %routed.rel, "record_post_edit_file failed");
                    }
                }
            }
        }
        "tool-use-failure" => {
            if let Some(sid) = session_id_field {
                let _ = state::ensure_session(&project_root, sid, agent, event.model.as_deref());
                let _ = state::record_hook_event(
                    &project_root,
                    sid,
                    event_name,
                    Some(event.extra.clone()),
                );
                if !project_root.is_empty() {
                    let tool_name = event
                        .extra
                        .get("tool_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let _ = state::record_tool_failure(&project_root, sid, tool_name);
                    let _ = state::record_tool_call(
                        &project_root,
                        sid,
                        tool_name,
                        event.extra.get("tool_input").cloned(),
                        event.extra.get("tool_output").cloned(),
                        true,
                    );
                }
            }
        }
        "subagent-start" => {
            if let Some(sid) = session_id_field {
                let _ = state::ensure_session(&project_root, sid, agent, event.model.as_deref());
                // Claude uses agent_id, Cursor uses subagent_id.
                let agent_id = event
                    .extra
                    .get("agent_id")
                    .or_else(|| event.extra.get("subagent_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                // Claude uses agent_type, Cursor uses subagent_type.
                let agent_type = event
                    .extra
                    .get("agent_type")
                    .or_else(|| event.extra.get("subagent_type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                if !agent_id.is_empty() {
                    let _ = state::record_subagent_start(&project_root, sid, agent_id, agent_type);
                }
            }
        }
        "after-agent-thought" => {
            if let Some(sid) = session_id_field {
                let _ = state::ensure_session(&project_root, sid, agent, event.model.as_deref());
                let _ = state::record_hook_event(
                    &project_root,
                    sid,
                    event_name,
                    Some(event.extra.clone()),
                );
                let duration_ms = event
                    .extra
                    .get("duration_ms")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                if duration_ms > 0 {
                    let _ = state::record_thinking(&project_root, sid, duration_ms);
                }
            }
        }
        "after-agent-response" => {
            if let Some(sid) = session_id_field {
                let _ = state::ensure_session(&project_root, sid, agent, event.model.as_deref());
                let _ = state::record_hook_event(
                    &project_root,
                    sid,
                    event_name,
                    Some(event.extra.clone()),
                );
                let _ = state::touch_session(&project_root, sid, None);
            }
        }
        "pre-compact" => {
            if let Some(sid) = session_id_field {
                let _ = state::ensure_session(&project_root, sid, agent, event.model.as_deref());
                let _ = state::record_hook_event(
                    &project_root,
                    sid,
                    event_name,
                    Some(event.extra.clone()),
                );
                let _ = state::record_compact(&project_root, sid);
            }
        }
        "stop" => {
            if let Some(sid) = session_id_field {
                if let Err(e) =
                    state::ensure_session(&project_root, sid, agent, event.model.as_deref())
                {
                    tracing::warn!(%e, session = sid, "stop: ensure_session failed");
                }
                if let Err(e) = state::update_session_metrics(
                    &project_root,
                    sid,
                    event.loop_count,
                    event.context_tokens,
                    event.context_window_size,
                ) {
                    tracing::warn!(%e, session = sid, "stop: update_session_metrics failed");
                }
                let transcript_path = event.extra.get("transcript_path").and_then(|v| v.as_str());
                if let Err(e) = state::record_hook_event(
                    &project_root,
                    sid,
                    event_name,
                    Some(event.extra.clone()),
                ) {
                    tracing::warn!(%e, session = sid, "stop: record_hook_event failed");
                }
                state::touch_session(&project_root, sid, transcript_path)?;

                if !project_root.is_empty() {
                    let files = if is_cursor_agent(agent) {
                        let db_files = crate::tools::cursor::composer_data::files_edited_in_session(
                            sid,
                            &project_root,
                            0,
                        );
                        if db_files.is_empty() {
                            state::get_edited_files(&project_root, sid)
                        } else {
                            db_files
                        }
                    } else {
                        let tracked = state::get_edited_files(&project_root, sid);
                        if tracked.is_empty() {
                            dirty_worktree_files(&project_root)
                        } else {
                            tracked
                        }
                    };
                    if !files.is_empty() {
                        let _ = state::snapshot_session_files(&project_root, sid, &files);
                    }
                    let finished = state::finish_turn(
                        &project_root,
                        sid,
                        agent,
                        event.model.as_deref(),
                        transcript_path,
                    )
                    .ok()
                    .flatten();
                    // Agents commit MID-turn: the commit's drain ran
                    // before this turn's snapshot existed, so the v2
                    // anchor is missing the just-finished turn.
                    //
                    // A full respool would re-enrich the anchor and
                    // claim the new turn's snapshot in `anchor.turns`,
                    // hiding the shadow anchor from the feed even
                    // though its content isn't in this commit. Instead,
                    // do a targeted v2-only backfill that writes the
                    // missing provenance turn and patches the anchor's
                    // session_refs without touching `anchor.turns`.
                    if finished.is_some() {
                        if let Some(anchor_turns) =
                            head_anchor_turn_count(&project_root, agent, sid)
                        {
                            let session_turns = crate::hooks::store::read(&project_root, sid)
                                .map_or(0, |s| s.current_turn_index as usize);
                            if anchor_turns < session_turns {
                                backfill_v2_turns(&project_root, agent, sid);
                            }
                        }
                    }
                }
                // Foreign-repo turns are finished regardless of whether the
                // origin is a git repo — their provenance lives in the
                // edited repos themselves. Same mid-turn-commit rule per repo.
                let foreign =
                    state::finish_foreign_turns(&project_root, sid, agent, event.model.as_deref())
                        .unwrap_or_default();
                for (repo_root, _) in &foreign {
                    if let Some(anchor_turns) = head_anchor_turn_count(repo_root, agent, sid) {
                        // Use the foreign repo's own turn index (how many turns
                        // touched IT), not the origin session's total turn count.
                        let foreign_turns = crate::hooks::store::read(&project_root, sid)
                            .and_then(|s| s.foreign_repos)
                            .and_then(|repos| repos.get(repo_root).map(|c| c.turn_index as usize))
                            .unwrap_or(0);
                        if anchor_turns < foreign_turns {
                            backfill_v2_turns(repo_root, agent, sid);
                        }
                    }
                }
            }
        }
        "subagent-stop" => {
            if let Some(sid) = session_id_field {
                let _ = state::ensure_session(&project_root, sid, agent, event.model.as_deref());
                let _ = state::record_hook_event(
                    &project_root,
                    sid,
                    event_name,
                    Some(event.extra.clone()),
                );
                state::touch_session(&project_root, sid, None)?;

                let agent_id = event
                    .extra
                    .get("agent_id")
                    .or_else(|| event.extra.get("subagent_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !agent_id.is_empty() {
                    let _ = state::record_subagent_end(&project_root, sid, agent_id);
                }

                if let Some(files_arr) = event
                    .extra
                    .get("modified_files")
                    .and_then(serde_json::Value::as_array)
                {
                    // Route subagent-reported files by their owning repo.
                    let mut origin_files: Vec<String> = Vec::new();
                    let mut foreign_files: std::collections::HashMap<String, Vec<String>> =
                        std::collections::HashMap::new();
                    for routed in files_arr
                        .iter()
                        .filter_map(|v| v.as_str())
                        .filter_map(|p| routing::route_file(p, &project_root))
                    {
                        if routed.foreign {
                            foreign_files
                                .entry(routed.repo_root)
                                .or_default()
                                .push(routed.rel);
                        } else {
                            origin_files.push(routed.rel);
                        }
                    }
                    if !origin_files.is_empty() {
                        let _ = state::snapshot_session_files(&project_root, sid, &origin_files);
                    }
                    for (repo_root, files) in &foreign_files {
                        let _ = state::snapshot_files_in_repo(&project_root, sid, repo_root, files);
                    }
                }
                if !project_root.is_empty() {
                    if let Err(e) =
                        state::finish_turn(&project_root, sid, agent, event.model.as_deref(), None)
                    {
                        tracing::warn!(%e, session = sid, "session-end: finish_turn failed");
                    }
                }
                if let Err(e) =
                    state::finish_foreign_turns(&project_root, sid, agent, event.model.as_deref())
                {
                    tracing::warn!(%e, session = sid, "session-end: finish_foreign_turns failed");
                }
            }
        }
        _ => {
            tracing::warn!(
                event = event_name,
                tool = agent,
                "unknown agent event, ignored"
            );
        }
    }

    Ok(())
}

fn summarize_tool_input_hook(
    tool_name: &str,
    tool_input: Option<&serde_json::Value>,
) -> Option<String> {
    crate::utils::summarize_tool_input(tool_name, tool_input, 200)
}

fn dirty_worktree_files(project_root: &str) -> Vec<String> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut files = std::collections::HashSet::new();

    // Modified tracked files (staged + unstaged)
    let mut cmd = std::process::Command::new(&git);
    cmd.args(["diff", "--name-only", "HEAD"])
        .current_dir(project_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    crate::git::proxy::scrub_git_env(&mut cmd);
    if let Ok(o) = cmd.output() {
        if o.status.success() {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                if !line.is_empty() {
                    files.insert(line.to_string());
                }
            }
        }
    }

    // Untracked files (new files created by the agent)
    let mut cmd = std::process::Command::new(&git);
    cmd.args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(project_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    crate::git::proxy::scrub_git_env(&mut cmd);
    if let Ok(o) = cmd.output() {
        if o.status.success() {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                if !line.is_empty() {
                    files.insert(line.to_string());
                }
            }
        }
    }

    files.into_iter().collect()
}

fn is_cursor_agent(agent: &str) -> bool {
    crate::core::tool::is_cursor_agent(agent)
}

fn is_read_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "Read"
            | "ReadFile"
            | "View"
            | "read_file"
            | "read"
            | "Grep"
            | "grep"
            | "Search"
            | "search"
            | "SemanticSearch"
            | "Glob"
            | "glob"
            | "ListFiles"
            | "list_files"
    )
}

fn is_dir_scoped_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "Grep"
            | "grep"
            | "Search"
            | "search"
            | "SemanticSearch"
            | "Glob"
            | "glob"
            | "ListFiles"
            | "list_files"
    )
}

fn is_file_mutating_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "Write"
            | "Edit"
            | "MultiEdit"
            | "StrReplace"
            | "Delete"
            | "write"
            | "edit"
            | "str_replace"
            | "delete"
            | "create_file"
            | "edit_file"
            | "edit_file_v2"
    )
}

/// Append a mutation that routed to NO git repo to the global ledger
/// (`$OOBO_HOME/state/no-repo-ledger.jsonl`). The capture contract is
/// "never silently discard": these edits have no store to live in, but
/// the fact that they happened is still evidence.
fn record_no_repo_touch(session_id: &str, agent: &str, tool_name: &str, path: &str) {
    // Non-file targets the router rejects for shape (not location).
    if path.is_empty() || path == "." || path.ends_with('/') {
        return;
    }
    let dir = crate::paths::oobo_home().join("state");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let entry = serde_json::json!({
        "session_id": session_id,
        "agent": agent,
        "tool_name": tool_name,
        "path": path,
        "timestamp": chrono::Utc::now().timestamp(),
    });
    use std::io::Write as _;
    let ledger_path = dir.join("no-repo-ledger.jsonl");
    let is_new = !ledger_path.exists();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ledger_path)
    {
        let _ = writeln!(f, "{entry}");
        #[cfg(unix)]
        if is_new {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&ledger_path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

/// Returns how many provenance turns HEAD's v2 anchor has for this
/// session, or `None` if the anchor doesn't reference it at all.
fn head_anchor_turn_count(repo_root: &str, agent: &str, session_id: &str) -> Option<usize> {
    if repo_root.is_empty() {
        return None;
    }
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut cmd = std::process::Command::new(git);
    cmd.args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .current_dir(repo_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    crate::git::proxy::scrub_git_env(&mut cmd);
    let Ok(out) = cmd.output() else {
        return None;
    };
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let canon = std::fs::canonicalize(repo_root).map_or_else(
        |_| repo_root.to_string(),
        |p| p.to_string_lossy().to_string(),
    );
    let repo_id = crate::project::id_for_root(&canon);
    let suid = crate::core::identity::session_uid(agent, session_id);
    let anchor = crate::git::orphan::v2::read_anchor(repo_root, &repo_id, &sha)?;
    anchor
        .session_refs
        .iter()
        .find(|r| r.session_uid == suid)
        .map(|r| r.turn_uids.len())
}

/// Targeted v2 backfill: write the missing provenance turn(s) and update
/// the anchor's session_refs without re-enriching the full anchor.
///
/// This is the safe alternative to `respool_after_turn` for mid-turn
/// commits in multi-turn sessions: the anchor's portable `turns` array
/// is NOT touched (preventing the feed from hiding shadow anchors for
/// content that isn't in the commit).
fn backfill_v2_turns(repo_root: &str, agent: &str, session_id: &str) {
    if repo_root.is_empty() || !crate::project_config::is_enabled(repo_root) {
        return;
    }
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut cmd = std::process::Command::new(git);
    cmd.args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .current_dir(repo_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    crate::git::proxy::scrub_git_env(&mut cmd);
    let Ok(out) = cmd.output() else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let canon = std::fs::canonicalize(repo_root).map_or_else(
        |_| repo_root.to_string(),
        |p| p.to_string_lossy().to_string(),
    );
    let repo_id = crate::project::id_for_root(&canon);
    let suid = crate::core::identity::session_uid(agent, session_id);

    let Some(mut anchor) = crate::git::orphan::v2::read_anchor(repo_root, &repo_id, &sha) else {
        return;
    };

    let Some(sref) = anchor
        .session_refs
        .iter_mut()
        .find(|r| r.session_uid == suid)
    else {
        return;
    };

    let existing: std::collections::HashSet<String> = sref.turn_uids.iter().cloned().collect();
    let snapshots = crate::git::turns::list_turn_snapshots(repo_root);

    // Read session state ONCE and reuse throughout the function.
    let session_state = crate::hooks::store::read(repo_root, session_id);
    let transcript_path = session_state
        .as_ref()
        .and_then(|s| s.transcript_path.clone());
    let cfg = crate::config::Config::load_or_default();
    crate::git::interceptor::ingest_turns_for_session(
        &cfg,
        repo_root,
        agent,
        session_id,
        transcript_path.as_deref(),
    );
    let tap_turns = crate::attribution::turn_store::read_all_turns(repo_root);
    let ts_of = crate::attribution::turn_store::turn_ts_secs;

    const JOIN_SLACK_SECS: i64 = 2;
    let mut new_uids = Vec::new();

    for snap in snapshots.iter().filter(|t| t.session_id == session_id) {
        let tuid = crate::core::identity::turn_uid(&suid, session_id, snap.turn_index);
        if existing.contains(&tuid) {
            continue;
        }
        let wstart = snap.started_at.unwrap_or(snap.created_at) - JOIN_SLACK_SECS;
        let wend = snap.ended_at.unwrap_or(snap.created_at) + JOIN_SLACK_SECS;

        let mut tokens = crate::core::turn::TurnTokens::default();
        let mut model: Option<String> = None;
        let mut tool_names: Vec<String> = Vec::new();
        let mut found_time_calls = false;
        for call in tap_turns.iter().filter(|t| {
            t.session_id == session_id
                && t.role == crate::core::turn::TurnRole::Assistant
                && ts_of(t).is_some_and(|ts| ts >= wstart && ts <= wend)
        }) {
            found_time_calls = true;
            tokens.accumulate(&call.tokens);
            if call.model.is_some() {
                model.clone_from(&call.model);
            }
            if let Some(names) = &call.tool_names {
                tool_names.extend(names.split(',').filter(|n| !n.is_empty()).map(String::from));
            }
        }
        // Timestamp-less taps: degrade to the index join (only sound when
        // the tap indexes per oobo turn). Mirrors worker::write_v2.
        if !found_time_calls {
            if let Some(t) = tap_turns.iter().find(|t| {
                t.session_id == session_id
                    && t.turn_index == snap.turn_index
                    && t.role == crate::core::turn::TurnRole::Assistant
                    && ts_of(t).is_none()
            }) {
                tokens = t.tokens;
                model.clone_from(&t.model);
                tool_names = t
                    .tool_names
                    .clone()
                    .map(|names| names.split(',').map(str::to_string).collect())
                    .unwrap_or_default();
            }
        }

        let trigger = tap_turns
            .iter()
            .filter(|t| {
                t.session_id == session_id
                    && t.role == crate::core::turn::TurnRole::User
                    && t.message_preview.is_some()
                    && ts_of(t).is_some_and(|ts| ts <= wend)
            })
            .max_by_key(|t| (ts_of(t), t.turn_index))
            .and_then(|t| t.message_preview.clone());

        let turn = crate::git::orphan::v2::TurnRecord {
            schema_version: crate::git::orphan::v2::V2_SCHEMA_VERSION,
            turn_uid: tuid.clone(),
            session_uid: suid.clone(),
            turn_index: snap.turn_index,
            native_turn_index: Some(snap.turn_index),
            source: snap.source.clone(),
            model,
            trigger,
            started_at: snap.started_at,
            ended_at: snap.ended_at,
            tokens,
            tool_names,
            capture_gap: snap.files.iter().any(|f| f.capture_gap),
        };
        let edits = crate::git::orphan::v2::TurnEdits {
            files: snap.files.clone(),
        };
        if let Err(e) =
            crate::git::orphan::v2::write_provenance_turn(repo_root, &repo_id, &turn, &edits)
        {
            tracing::warn!(%e, turn_uid = %tuid, "backfill: provenance turn write failed");
        }

        // Conversation layer: transcript + tool calls at the session's
        // home repo ONLY (never write transcripts into foreign repos).
        let is_home = session_state
            .as_ref()
            .and_then(|s| s.worktree.as_deref())
            .is_none_or(|origin| {
                let canon_origin = std::fs::canonicalize(origin)
                    .map_or(origin.to_string(), |p| p.to_string_lossy().to_string());
                canon == canon_origin
            });
        if is_home
            && anchor.anchor.transparency_mode == crate::core::anchor::TransparencyMode::On
            && (snap.memory.transcript.is_some() || !snap.memory.tool_calls.is_empty())
        {
            let transcript_json = serde_json::json!({
                "schema_version": crate::git::orphan::v2::V2_SCHEMA_VERSION,
                "turn_uid": tuid,
                "session_uid": suid,
                "turn_index": snap.turn_index,
                "native_transcript_path": snap.memory.transcript_path,
                "transcript": snap.memory.transcript,
            })
            .to_string();
            let tool_calls_json =
                serde_json::to_string(&snap.memory.tool_calls).unwrap_or_else(|_| "[]".into());
            if let Err(e) = crate::git::orphan::v2::write_conversation_turn(
                repo_root,
                &suid,
                snap.turn_index,
                &transcript_json,
                &tool_calls_json,
            ) {
                tracing::warn!(%e, turn_index = snap.turn_index, "backfill: conversation turn write failed");
            }
        }

        new_uids.push(tuid);
    }

    if new_uids.is_empty() {
        return;
    }

    // Update the session record's turn_count to reflect the new turns.
    let new_turn_count = snapshots
        .iter()
        .filter(|t| t.session_id == session_id)
        .map(|t| t.turn_index + 1)
        .max()
        .unwrap_or(0);
    if new_turn_count > 0 {
        let tool = crate::core::tool::normalize_source(agent).to_string();
        let updated_record = crate::git::orphan::v2::SessionRecord {
            schema_version: crate::git::orphan::v2::V2_SCHEMA_VERSION,
            session_uid: suid.clone(),
            native_session_ids: vec![session_id.to_string()],
            tool,
            model: session_state.as_ref().and_then(|s| s.model.clone()),
            home_location: None,
            origin_repo_id: Some(repo_id.clone()),
            repos_touched: vec![repo_id.clone()],
            lineage: crate::core::identity::SessionLineage::default(),
            turn_count: new_turn_count,
            title: None,
            started_at: session_state.as_ref().map_or(0, |s| s.started_at),
            updated_at: session_state.as_ref().map_or(0, |s| s.updated_at),
            ended_at: session_state.as_ref().and_then(|s| s.ended_at),
        };
        if let Err(e) =
            crate::git::orphan::v2::write_provenance_session(repo_root, &repo_id, &updated_record)
        {
            tracing::warn!(%e, session_uid = %suid, "backfill: provenance session write failed");
        }
    }

    // Patch the anchor's session_refs with the new turn UIDs.
    sref.turn_uids.extend(new_uids);
    if let Err(e) = crate::git::orphan::v2::write_anchor(repo_root, &repo_id, &anchor, None) {
        tracing::warn!(%e, "backfill: anchor update failed");
    }
    crate::git::anchor_cache::invalidate(repo_root);

    // Push the updated v2 branch so backfilled turns aren't stranded
    // locally when the session's last turn was itself a push.
    if crate::git::orphan::v2::branch_exists(repo_root) {
        if let Err(e) = crate::git::orphan::v2::push(repo_root) {
            tracing::warn!(%e, "backfill: v2 push failed");
        }
    }
}

/// session-start self-heal: if this enabled repo lost its git hooks
/// (hook file deleted, `core.hooksPath` reset, fresh clone), reinstall
/// them. Cheap fast-path: skip when post-commit already mentions oobo.
fn self_heal_project_hooks(project_root: &str) {
    if !crate::project_config::is_enabled(project_root) {
        return;
    }
    let hook = crate::git::detect::resolve_git_common_dir(project_root)
        .join("hooks")
        .join("post-commit");
    let healthy = std::fs::read_to_string(&hook)
        .map(|c| c.contains("oobo"))
        .unwrap_or(false);
    if healthy {
        return;
    }
    match install::install_project_hooks(project_root) {
        Ok(_) => tracing::info!(root = project_root, "self-healed missing git hooks"),
        Err(e) => tracing::warn!(root = project_root, %e, "git hook self-heal failed"),
    }
}

/// Strip the project root prefix to get a relative path, matching how git
/// and the rest of the attribution pipeline represent file paths.
/// Canonicalizes both sides to handle symlinks and macOS `/var` vs `/private/var`.
fn make_relative(abs_path: &str, project_root: &str) -> String {
    let abs =
        std::fs::canonicalize(abs_path).unwrap_or_else(|_| std::path::PathBuf::from(abs_path));
    let root = std::fs::canonicalize(project_root)
        .unwrap_or_else(|_| std::path::PathBuf::from(project_root));
    abs.strip_prefix(&root).map_or_else(
        |_| abs_path.to_string(),
        |p| {
            let s = p.to_string_lossy().to_string();
            s.replace('\\', "/")
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_event_deserialize() {
        let json = r#"{"session_id": "s1", "agent": "cursor", "model": "claude-opus-4"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.session_id.as_deref(), Some("s1"));
        assert_eq!(event.agent.as_deref(), Some("cursor"));
        assert_eq!(event.model.as_deref(), Some("claude-opus-4"));
    }

    #[test]
    fn test_hook_event_deserialize_minimal() {
        let event: HookEvent = serde_json::from_str("{}").unwrap();
        assert!(event.session_id.is_none());
        assert!(event.event.is_empty());
        assert!(event.cwd.is_none());
        assert!(event.agent.is_none());
        assert!(event.model.is_none());
    }

    #[test]
    fn test_hook_event_with_extra_fields() {
        let json = r#"{"session_id": "s1", "unknown_field": 42}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.session_id.as_deref(), Some("s1"));
        assert_eq!(
            event
                .extra
                .get("unknown_field")
                .and_then(serde_json::Value::as_i64),
            Some(42)
        );
    }

    #[test]
    fn test_handle_event_session_start() {
        let result = handle_event("session-start", "{}", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_event_malformed_json_returns_ok() {
        let result = handle_event("session-start", "not json", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_event_session_end_no_id() {
        let result = handle_event("session-end", "{}", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_tool_flag_overrides_payload_agent() {
        let json = r#"{"session_id": "s1", "agent": "agent"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.agent.as_deref(), Some("agent"));
    }

    #[test]
    fn test_make_relative_strips_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let file = root.join("src/main.rs");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(&file, "fn main() {}").unwrap();

        let result = make_relative(file.to_str().unwrap(), root.to_str().unwrap());
        assert_eq!(result, "src/main.rs");
    }

    #[test]
    fn test_make_relative_outside_project() {
        let result = make_relative("/tmp/other/file.rs", "/home/user/project");
        assert_eq!(result, "/tmp/other/file.rs");
    }

    #[test]
    fn test_is_read_tool_positives() {
        for tool in [
            "Read",
            "ReadFile",
            "View",
            "read_file",
            "read",
            "Grep",
            "grep",
            "Search",
            "search",
            "SemanticSearch",
            "Glob",
            "glob",
            "ListFiles",
            "list_files",
        ] {
            assert!(is_read_tool(tool), "{tool} should be a read tool");
        }
    }

    #[test]
    fn test_is_read_tool_negatives() {
        for tool in ["Write", "Edit", "Bash", "Shell", "Delete", "cat", ""] {
            assert!(!is_read_tool(tool), "{tool} should NOT be a read tool");
        }
    }

    #[test]
    fn test_is_dir_scoped_tool() {
        assert!(is_dir_scoped_tool("Grep"));
        assert!(is_dir_scoped_tool("Search"));
        assert!(is_dir_scoped_tool("Glob"));
        assert!(!is_dir_scoped_tool("Read"));
        assert!(!is_dir_scoped_tool("View"));
    }

    #[test]
    fn test_handle_event_unknown_event_no_error() {
        let json = r#"{"session_id": "s1"}"#;
        let result = handle_event("totally-unknown-event", json, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_event_missing_workspace_and_cwd() {
        let json = r#"{"session_id": "s1", "event": "session-end"}"#;
        let result = handle_event("session-end", json, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_event_empty_payload_returns_ok() {
        let result = handle_event("session-end", "", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_event_truncated_json_returns_ok() {
        let result = handle_event("session-start", r#"{"session_id": "#, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_dir_scoped_is_subset_of_read_tool() {
        for tool in [
            "Grep",
            "grep",
            "Search",
            "search",
            "SemanticSearch",
            "Glob",
            "glob",
            "ListFiles",
            "list_files",
        ] {
            assert!(
                is_read_tool(tool),
                "{tool} is in is_dir_scoped_tool but not is_read_tool  --  \
                 the dir-scoped check will never fire"
            );
        }
    }
}
