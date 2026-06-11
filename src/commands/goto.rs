use crate::cli::OutputMode;
use crate::config::Config;
use crate::core::turn::TurnSnapshot;
use crate::error::{CliError, CmdResult};

/// State file recording the navigation stack, so `oobo back` can return.
const GOTO_STATE_FILE: &str = "oobo-state/goto-stack.json";

/// `oobo goto <id>`  --  travel to a turn or commit.
///
/// Safety guarantees:
/// 1. If the worktree is dirty, we auto-stash (unless `--no-stash`).
/// 2. We record the current HEAD + stash ref so `oobo back` can return.
/// 3. `read-tree --reset -u` atomically replaces the worktree.
pub fn run(cfg: &Config, target: &str, no_stash: bool, mode: OutputMode) -> CmdResult {
    let Some(project_root) = crate::git::proxy::project_root(cfg) else {
        eprintln!("oobo: not inside a git repository.");
        return Ok(1);
    };

    // Resolve target: try as turn ID first, then as commit SHA.
    let resolved = resolve_target(&project_root, target);
    let (tree, label, kind) = match resolved {
        Target::Turn(turn) => {
            let tree = match turn.tree_hash.as_deref() {
                Some(t) if !t.is_empty() => t.to_string(),
                _ => {
                    eprintln!("oobo: turn '{}' has no restorable tree snapshot.", turn.id);
                    return Ok(1);
                }
            };
            let label = format!("turn {} ({})", short(&turn.id), turn.source);
            (tree, label, GotoKind::Turn(turn))
        }
        Target::Commit(sha, subject) => {
            let tree = format!("{sha}^{{tree}}");
            let label = subject
                .as_deref()
                .map_or_else(|| format!("commit {}", short(&sha)), ToString::to_string);
            (tree, label, GotoKind::Commit(sha))
        }
        Target::NotFound => {
            eprintln!("oobo: no turn or commit found for '{target}'.");
            return Ok(1);
        }
        Target::Ambiguous(matches) => {
            eprintln!(
                "oobo: '{target}' is ambiguous  --  matches {} turns:",
                matches.len()
            );
            for t in &matches {
                eprintln!(
                    "  {}  {}:{}",
                    t.id,
                    t.source,
                    &t.session_id[..8.min(t.session_id.len())]
                );
            }
            return Ok(1);
        }
    };

    // Safety: handle dirty worktree.
    // Only stash on the first goto (empty stack). When already navigating,
    // the worktree was set by a previous goto  --  nothing user-owned to save.
    let is_first_goto = stack_depth(&project_root) == 0;
    let stash_ref = if is_first_goto {
        let dirty = has_dirty_changes(&project_root)?;
        if dirty {
            if no_stash {
                eprintln!("oobo: worktree has uncommitted changes and --no-stash was set.");
                return Ok(1);
            }
            let stash = create_stash(&project_root)?;
            Some(stash)
        } else {
            None
        }
    } else {
        None
    };

    // Record return point BEFORE modifying the worktree.
    // write-tree captures the current index (which is the state we want to restore on `back`).
    let current_tree = git_capture(&project_root, &["write-tree"])?;
    // Label: describes what we're leaving (shown on `back`).
    let leaving_label = if is_first_goto {
        // Leaving HEAD  --  use its commit message.
        git_capture(&project_root, &["show", "-s", "--format=%s", "HEAD"])
            .unwrap_or_else(|_| "HEAD".into())
    } else {
        // Leaving a previously-loaded target. Read the last stack entry's
        // "going_to" label, or fall back to a generic description.
        let stack = load_stack(&project_root);
        stack
            .entries
            .last()
            .and_then(|e| e.went_to_label.clone())
            .unwrap_or_else(|| "previous state".into())
    };
    push_stack(
        &project_root,
        &current_tree,
        &leaving_label,
        stash_ref.as_deref(),
        &label,
    )?;

    // Load the target tree.
    git_capture(&project_root, &["read-tree", "--reset", "-u", &tree])?;

    // Mark lineage for the anchor system.
    let restore_id = match &kind {
        GotoKind::Turn(t) => t.id.clone(),
        GotoKind::Commit(sha) => format!("anchor:{sha}"),
    };
    crate::hooks::state::mark_restored_from(&project_root, &restore_id)
        .map_err(|e| CliError::Git(format!("mark restored: {e}")))?;

    // Materialize turn memory if applicable.
    let memory_path = if let GotoKind::Turn(ref turn) = kind {
        materialize_turn_memory(&project_root, turn)?
    } else {
        None
    };

    // goto is STRICTLY repo-local: we restored only this repo's slice.
    // If the turn's session also edited other repos, say so — those
    // worktrees are informational context, never a goto target.
    if let GotoKind::Turn(ref turn) = kind {
        for foreign in foreign_repos_of_session(&project_root, &turn.session_id) {
            eprintln!(
                "note: this session also edited {foreign} — \
                 goto is repo-local, that worktree is untouched."
            );
        }
    }

    emit_success(&label, stash_ref.is_some(), memory_path.as_deref(), mode);
    Ok(0)
}

/// Other repos this session's live capture state has touched (empty when
/// the session is gone — the note is best-effort context, not data).
fn foreign_repos_of_session(project_root: &str, session_id: &str) -> Vec<String> {
    crate::hooks::store::list_for_project(project_root)
        .iter()
        .find(|s| s.session_id == session_id)
        .and_then(|s| s.foreign_repos.as_ref())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// `oobo back`  --  pop one level from the navigation stack.
pub fn run_back(cfg: &Config, mode: OutputMode) -> CmdResult {
    let Some(project_root) = crate::git::proxy::project_root(cfg) else {
        eprintln!("oobo: not inside a git repository.");
        return Ok(1);
    };

    let depth = stack_depth(&project_root);
    if depth == 0 {
        eprintln!("oobo: nothing to go back to. Use `oobo goto <id>` first.");
        return Ok(1);
    }

    // Check if the user has made changes ON TOP of the goto state.
    let wt_dirty = git_capture(&project_root, &["diff", "--quiet"]);
    let untracked = git_capture(
        &project_root,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "--exclude=.oobo/",
        ],
    )
    .map(|s| {
        s.lines()
            .any(|l| !l.trim().is_empty() && !l.starts_with(".oobo/") && !l.starts_with(".oobo\\"))
    })
    .unwrap_or(false);

    if wt_dirty.is_err() || untracked {
        eprintln!("oobo: you have new changes since `goto`. Commit or stash them first.");
        return Ok(1);
    }

    let Some(entry) = pop_stack(&project_root) else {
        eprintln!("oobo: navigation stack unexpectedly empty.");
        return Ok(1);
    };
    let remaining = stack_depth(&project_root);

    // Restore the tree we came from.
    git_capture(&project_root, &["read-tree", "--reset", "-u", &entry.tree])?;

    // Pop the stash if this entry created one.
    if let Some(ref stash) = entry.stash_ref {
        let pop_result = git_capture(&project_root, &["stash", "apply", stash]);
        match pop_result {
            Ok(_) => {
                let _ = git_capture(&project_root, &["stash", "drop", stash]);
            }
            Err(e) => {
                eprintln!(
                    "oobo: restored tree but stash apply had conflicts: {e}\n\
                     Your stash is still saved. Run `git stash pop` to resolve manually."
                );
            }
        }
    }

    match mode {
        OutputMode::Json => crate::utils::print_json(&serde_json::json!({
            "action": "back",
            "label": entry.label,
            "stash_applied": entry.stash_ref.is_some(),
            "remaining_depth": remaining,
        })),
        OutputMode::Agent => {
            let depth_hint = if remaining > 0 {
                format!(" ({remaining} more in stack)")
            } else {
                String::new()
            };
            println!("back to {}{depth_hint}", entry.label);
        }
        OutputMode::Tui => {
            if entry.stash_ref.is_some() {
                println!(
                    "Returned to {} (uncommitted changes restored).",
                    entry.label
                );
            } else {
                println!("Returned to {}.", entry.label);
            }
            if remaining > 0 {
                println!("  {remaining} more in history  --  run `oobo back` again to keep going.");
            }
        }
    }
    Ok(0)
}

// ── Internals ────────────────────────────────────────────────────────────

enum Target {
    Turn(Box<TurnSnapshot>),
    Commit(String, Option<String>),
    NotFound,
    Ambiguous(Vec<TurnSnapshot>),
}

enum GotoKind {
    Turn(Box<TurnSnapshot>),
    Commit(String),
}

fn resolve_target(project_root: &str, id: &str) -> Target {
    // Try exact turn match first.
    if let Some(turn) = crate::git::turns::read_turn_snapshot(project_root, id) {
        return Target::Turn(Box::new(turn));
    }
    // Prefix match on turns.
    let matches: Vec<TurnSnapshot> = crate::git::turns::list_turn_snapshots(project_root)
        .into_iter()
        .filter(|t| t.id.starts_with(id))
        .collect();
    if matches.len() == 1 {
        return Target::Turn(Box::new(matches.into_iter().next().unwrap()));
    }
    if matches.len() > 1 {
        return Target::Ambiguous(matches);
    }
    // Try as git commit.
    if let Ok(sha) = git_capture(
        project_root,
        &["rev-parse", "--verify", &format!("{id}^{{commit}}")],
    ) {
        let subject = git_capture(project_root, &["show", "-s", "--format=%s", &sha]).ok();
        return Target::Commit(sha, subject);
    }
    Target::NotFound
}

fn has_dirty_changes(project_root: &str) -> Result<bool, CliError> {
    let status = git_capture(project_root, &["status", "--porcelain"])?;
    Ok(!status.trim().is_empty())
}

fn create_stash(project_root: &str) -> Result<String, CliError> {
    git_capture(
        project_root,
        &[
            "stash",
            "push",
            "-m",
            "oobo goto (auto-stash)",
            "--include-untracked",
        ],
    )?;
    // Get the stash ref.
    let stash_ref = git_capture(project_root, &["stash", "list", "--format=%gd", "-1"])?;
    if stash_ref.is_empty() {
        return Err(CliError::Git("stash created but ref not found".into()));
    }
    Ok(stash_ref)
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct StackEntry {
    /// The tree-ish we came FROM (what to restore on `back`).
    tree: String,
    /// Label for display on `back` (e.g. "anchor C" = what we left).
    label: String,
    /// Label of where we navigated TO from this entry (used for subsequent `back` labeling).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    went_to_label: Option<String>,
    /// Stash ref created when leaving this state (if worktree was dirty).
    stash_ref: Option<String>,
    timestamp: i64,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct NavigationStack {
    entries: Vec<StackEntry>,
}

fn state_path(project_root: &str) -> std::path::PathBuf {
    crate::git::detect::resolve_git_common_dir(project_root).join(GOTO_STATE_FILE)
}

fn push_stack(
    project_root: &str,
    tree: &str,
    label: &str,
    stash_ref: Option<&str>,
    went_to: &str,
) -> Result<(), CliError> {
    let path = state_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CliError::Io {
            context: "create goto state dir".into(),
            source: e,
        })?;
    }
    let mut stack = load_stack(project_root);
    stack.entries.push(StackEntry {
        tree: tree.to_string(),
        label: label.to_string(),
        went_to_label: Some(went_to.to_string()),
        stash_ref: stash_ref.map(str::to_string),
        timestamp: chrono::Utc::now().timestamp(),
    });
    save_stack(project_root, &stack)
}

fn pop_stack(project_root: &str) -> Option<StackEntry> {
    let mut stack = load_stack(project_root);
    let entry = stack.entries.pop();
    if stack.entries.is_empty() {
        let _ = std::fs::remove_file(state_path(project_root));
    } else {
        let _ = save_stack(project_root, &stack);
    }
    entry
}

fn load_stack(project_root: &str) -> NavigationStack {
    let path = state_path(project_root);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_stack(project_root: &str, stack: &NavigationStack) -> Result<(), CliError> {
    let path = state_path(project_root);
    let json = serde_json::to_string_pretty(stack)
        .map_err(|e| CliError::Git(format!("serialize goto stack: {e}")))?;
    std::fs::write(&path, json).map_err(|e| CliError::Io {
        context: "write goto stack".into(),
        source: e,
    })?;
    Ok(())
}

fn stack_depth(project_root: &str) -> usize {
    load_stack(project_root).entries.len()
}

fn materialize_turn_memory(
    project_root: &str,
    turn: &TurnSnapshot,
) -> Result<Option<String>, CliError> {
    if turn.memory.transcript.is_none()
        && turn.memory.transcript_path.is_none()
        && turn.memory.hook_events.is_empty()
        && turn.memory.tool_calls.is_empty()
    {
        return Ok(None);
    }

    let dir = crate::git::detect::resolve_git_common_dir(project_root)
        .join("oobo-state")
        .join("goto");
    std::fs::create_dir_all(&dir).map_err(|e| CliError::Io {
        context: "create goto memory dir".into(),
        source: e,
    })?;
    let path = dir.join(format!("{}.json", safe_file_stem(&turn.id)));
    let payload = serde_json::json!({
        "schema_version": 1,
        "kind": "oobo_goto_memory",
        "turn_id": turn.id,
        "session_id": turn.session_id,
        "source": turn.source,
        "turn_index": turn.turn_index,
        "native_transcript_path": turn.memory.transcript_path.clone(),
        "transcript": turn.memory.transcript.clone(),
        "hook_events": turn.memory.hook_events.clone(),
        "tool_calls": turn.memory.tool_calls.clone(),
        "files": turn.files.clone(),
    });
    let text = serde_json::to_string_pretty(&payload)
        .map_err(|e| CliError::Git(format!("serialize goto memory: {e}")))?;
    std::fs::write(&path, text).map_err(|e| CliError::Io {
        context: "write goto memory".into(),
        source: e,
    })?;
    Ok(Some(path.to_string_lossy().to_string()))
}

fn emit_success(label: &str, stashed: bool, memory_path: Option<&str>, mode: OutputMode) {
    match mode {
        OutputMode::Json => crate::utils::print_json(&serde_json::json!({
            "action": "goto",
            "target": label,
            "stashed": stashed,
            "memory_path": memory_path,
        })),
        OutputMode::Agent => {
            let stash_note = if stashed { " (changes stashed)" } else { "" };
            println!("goto {label}{stash_note}");
            println!("run `oobo back` to return.");
        }
        OutputMode::Tui => {
            if stashed {
                println!("Loaded {label}. (uncommitted changes auto-stashed)");
            } else {
                println!("Loaded {label}.");
            }
            println!("Run `oobo back` to return to where you were.");
        }
    }
}

fn safe_file_stem(raw: &str) -> String {
    let safe: String = raw
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => c,
            _ => '_',
        })
        .collect();
    let safe = safe.trim_matches('.').trim_matches('_');
    if safe.is_empty() {
        "turn".to_string()
    } else {
        safe.to_string()
    }
}

fn short(hash: &str) -> &str {
    &hash[..hash.len().min(10)]
}

fn git_capture(project_root: &str, args: &[&str]) -> Result<String, CliError> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let output = std::process::Command::new(git)
        .args(args)
        .current_dir(project_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_QUARANTINE_PATH")
        .output()
        .map_err(|e| CliError::Git(format!("git: {e}")))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout)
            .replace('\r', "")
            .trim()
            .to_string())
    } else {
        Err(CliError::Git(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_hash_truncates() {
        assert_eq!(short("abcdef1234567890"), "abcdef1234");
        assert_eq!(short("abc"), "abc");
    }

    #[test]
    fn safe_file_stem_sanitizes() {
        assert_eq!(safe_file_stem("t:abc/123"), "t_abc_123");
        assert_eq!(safe_file_stem("..."), "turn");
    }

    #[test]
    fn navigation_stack_roundtrip() {
        let stack = NavigationStack {
            entries: vec![
                StackEntry {
                    tree: "tree1".into(),
                    label: "first commit".into(),
                    went_to_label: Some("second commit".into()),
                    stash_ref: Some("stash@{0}".into()),
                    timestamp: 1700000000,
                },
                StackEntry {
                    tree: "tree2".into(),
                    label: "second".into(),
                    went_to_label: None,
                    stash_ref: None,
                    timestamp: 1700000001,
                },
            ],
        };
        let json = serde_json::to_string(&stack).unwrap();
        let restored: NavigationStack = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.entries.len(), 2);
        assert_eq!(restored.entries[0].tree, "tree1");
        assert_eq!(restored.entries[0].stash_ref.as_deref(), Some("stash@{0}"));
        assert_eq!(
            restored.entries[0].went_to_label.as_deref(),
            Some("second commit")
        );
        assert_eq!(restored.entries[1].label, "second");
    }
}
