use crate::cli::OutputMode;
use crate::config::Config;
use crate::core::turn::TurnSnapshot;

pub fn run_turn(
    cfg: &Config,
    turn_id: &str,
    load: bool,
    force: bool,
    mode: OutputMode,
) -> Result<i32, String> {
    let Some(project_root) = crate::git::proxy::project_root(cfg) else {
        eprintln!("error: 'from turn' requires being inside a git repo.");
        return Ok(1);
    };

    let matches = resolve_turn(&project_root, turn_id);
    match matches.len() {
        0 => {
            eprintln!("error: no turn found for '{turn_id}'");
            return Ok(1);
        }
        1 => {}
        _ => {
            eprintln!("error: '{turn_id}' matches multiple turns:");
            for turn in &matches {
                eprintln!("  {}  {}:{}", turn.id, turn.source, turn.session_id);
            }
            return Ok(1);
        }
    }
    let turn = &matches[0];

    if !load {
        emit_preview(turn, mode);
        return Ok(0);
    }

    let dirty = dirty_status(&project_root)?;
    if !dirty.is_empty() && !force {
        eprintln!("error: worktree has uncommitted changes; rerun with --force to load anyway.");
        match mode {
            OutputMode::Json => crate::utils::print_json(&serde_json::json!({
                "action": "blocked",
                "reason": "dirty_worktree",
                "dirty": dirty,
            })),
            OutputMode::Agent => println!("blocked dirty_worktree"),
            OutputMode::Tui => println!("blocked: worktree has uncommitted changes"),
        }
        return Ok(1);
    }

    let tree = match turn.tree_hash.as_deref() {
        Some(tree) if !tree.is_empty() => tree,
        _ => {
            eprintln!("error: turn '{}' has no restorable tree", turn.id);
            return Ok(1);
        }
    };

    read_tree(&project_root, tree)?;
    let memory_path = materialize_turn_memory(&project_root, turn)?;
    crate::hooks::state::mark_restored_from(&project_root, &turn.id)
        .map_err(|e| format!("mark restored turn: {e}"))?;
    emit_loaded(turn, tree, memory_path.as_deref(), mode);
    Ok(0)
}

pub fn run_anchor(
    cfg: &Config,
    sha: &str,
    load: bool,
    force: bool,
    mode: OutputMode,
) -> Result<i32, String> {
    let Some(project_root) = crate::git::proxy::project_root(cfg) else {
        eprintln!("error: 'from anchor' requires being inside a git repo.");
        return Ok(1);
    };

    let commit = match git_capture(&project_root, &["rev-parse", "--verify", sha]) {
        Ok(commit) => commit,
        Err(_) => {
            eprintln!("error: no anchor/commit found for '{sha}'");
            return Ok(1);
        }
    };
    let subject = git_capture(&project_root, &["show", "-s", "--format=%s", &commit]).ok();

    if !load {
        emit_anchor_preview(&commit, subject.as_deref(), mode);
        return Ok(0);
    }

    let dirty = dirty_status(&project_root)?;
    if !dirty.is_empty() && !force {
        eprintln!("error: worktree has uncommitted changes; rerun with --force to load anyway.");
        match mode {
            OutputMode::Json => crate::utils::print_json(&serde_json::json!({
                "action": "blocked",
                "reason": "dirty_worktree",
                "dirty": dirty,
            })),
            OutputMode::Agent => println!("blocked dirty_worktree"),
            OutputMode::Tui => println!("blocked: worktree has uncommitted changes"),
        }
        return Ok(1);
    }

    read_tree(&project_root, &format!("{commit}^{{tree}}"))?;
    crate::hooks::state::mark_restored_from(&project_root, &format!("anchor:{commit}"))
        .map_err(|e| format!("mark restored anchor: {e}"))?;
    emit_anchor_loaded(&commit, mode);
    Ok(0)
}

fn emit_preview(turn: &TurnSnapshot, mode: OutputMode) {
    match mode {
        OutputMode::Json => crate::utils::print_json(&serde_json::json!({
            "action": "preview",
            "type": "turn",
            "load_required": true,
            "turn": turn_summary(turn),
        })),
        OutputMode::Agent => {
            println!(
                "from turn {} preview source={} session={} files={} load_required=true",
                turn.id,
                turn.source,
                turn.session_id,
                turn.files.len()
            );
        }
        OutputMode::Tui => {
            println!("turn {}", turn.id);
            println!("source:  {}", turn.source);
            println!("session: {}", turn.session_id);
            println!("files:   {}", turn.files.len());
            if let Some(tree) = &turn.tree_hash {
                println!("tree:    {tree}");
            }
            println!();
            println!(
                "preview only. run `anchor from turn {} --load` to load it.",
                turn.id
            );
        }
    }
}

fn emit_loaded(turn: &TurnSnapshot, tree: &str, memory_path: Option<&str>, mode: OutputMode) {
    match mode {
        OutputMode::Json => crate::utils::print_json(&serde_json::json!({
            "action": "loaded",
            "type": "turn",
            "turn": turn_summary(turn),
            "tree": tree,
            "memory_path": memory_path,
        })),
        OutputMode::Agent => match memory_path {
            Some(path) => println!("loaded turn {} tree={} memory={}", turn.id, tree, path),
            None => println!("loaded turn {} tree={}", turn.id, tree),
        },
        OutputMode::Tui => {
            println!("loaded turn {} into the worktree.", turn.id);
            if let Some(path) = memory_path {
                println!("memory: {path}");
            }
        }
    }
}

fn emit_anchor_preview(commit: &str, subject: Option<&str>, mode: OutputMode) {
    match mode {
        OutputMode::Json => crate::utils::print_json(&serde_json::json!({
            "action": "preview",
            "type": "anchor",
            "load_required": true,
            "commit": commit,
            "subject": subject,
        })),
        OutputMode::Agent => println!(
            "from anchor {} preview load_required=true",
            short_hash(commit)
        ),
        OutputMode::Tui => {
            println!("anchor {}", commit);
            if let Some(subject) = subject {
                println!("subject: {subject}");
            }
            println!();
            println!(
                "preview only. run `anchor from anchor {} --load` to load it.",
                short_hash(commit)
            );
        }
    }
}

fn emit_anchor_loaded(commit: &str, mode: OutputMode) {
    match mode {
        OutputMode::Json => crate::utils::print_json(&serde_json::json!({
            "action": "loaded",
            "type": "anchor",
            "commit": commit,
        })),
        OutputMode::Agent => println!("loaded anchor {}", short_hash(commit)),
        OutputMode::Tui => println!("loaded anchor {} into the worktree.", short_hash(commit)),
    }
}

fn turn_summary(turn: &TurnSnapshot) -> serde_json::Value {
    serde_json::json!({
        "id": turn.id,
        "source": turn.source,
        "session_id": turn.session_id,
        "turn_index": turn.turn_index,
        "parent_id": turn.parent_id,
        "restored_from": turn.restored_from,
        "tree_hash": turn.tree_hash,
        "files": turn.files.len(),
        "tool_calls": turn.memory.tool_calls.len(),
    })
}

fn materialize_turn_memory(
    project_root: &str,
    turn: &TurnSnapshot,
) -> Result<Option<String>, String> {
    if turn.memory.transcript.is_none()
        && turn.memory.transcript_path.is_none()
        && turn.memory.hook_events.is_empty()
        && turn.memory.tool_calls.is_empty()
    {
        return Ok(None);
    }

    let dir = crate::git::detect::resolve_git_common_dir(project_root)
        .join("oobo-state")
        .join("from");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create memory dir: {e}"))?;
    let path = dir.join(format!("{}.json", safe_file_stem(&turn.id)));
    let payload = serde_json::json!({
        "schema_version": 1,
        "kind": "oobo_from_turn_memory",
        "turn": turn_summary(turn),
        "session_id": turn.session_id,
        "source": turn.source,
        "native_transcript_path": turn.memory.transcript_path.clone(),
        "transcript": turn.memory.transcript.clone(),
        "hook_events": turn.memory.hook_events.clone(),
        "tool_calls": turn.memory.tool_calls.clone(),
        "files": turn.files.clone(),
    });
    let text = serde_json::to_string_pretty(&payload)
        .map_err(|e| format!("serialize turn memory: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("write turn memory: {e}"))?;
    Ok(Some(path.to_string_lossy().to_string()))
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

fn resolve_turn(project_root: &str, id_or_prefix: &str) -> Vec<TurnSnapshot> {
    if let Some(turn) = crate::git::turns::read_turn_snapshot(project_root, id_or_prefix) {
        return vec![turn];
    }
    crate::git::turns::list_turn_snapshots(project_root)
        .into_iter()
        .filter(|turn| turn.id == id_or_prefix || turn.id.starts_with(id_or_prefix))
        .collect()
}

fn dirty_status(project_root: &str) -> Result<Vec<String>, String> {
    let status = git_capture(project_root, &["status", "--porcelain"])?;
    Ok(status.lines().map(|line| line.to_string()).collect())
}

fn read_tree(project_root: &str, tree: &str) -> Result<(), String> {
    git_capture(project_root, &["read-tree", "--reset", "-u", tree]).map(|_| ())
}

fn git_capture(project_root: &str, args: &[&str]) -> Result<String, String> {
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
        .map_err(|e| format!("git: {e}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout)
            .replace('\r', "")
            .trim()
            .to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn short_hash(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_status_lines_are_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_str().unwrap();
        let init = std::process::Command::new("git")
            .args(["init", repo])
            .output()
            .unwrap();
        assert!(init.status.success());
        std::fs::write(tmp.path().join("file.txt"), "hello\n").unwrap();

        let dirty = dirty_status(repo).unwrap();
        assert_eq!(dirty, vec!["?? file.txt".to_string()]);
    }
}
