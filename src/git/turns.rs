use std::process::{Command, Stdio};

use crate::core::turn::{fnv1a64, TurnSnapshot};

/// Local-only, hidden Git ref namespace for full turn memory.
///
/// Anchors may reference these IDs, but sync/remote payloads must not assume
/// the full memory object is shareable. Exporting turn memory should go through
/// an explicit redaction/sync policy, not a blanket push of this namespace.
pub const REF_PREFIX: &str = "refs/oobo/turns/v1";

pub fn worktree_id(project_root: &str) -> String {
    let canonical = std::fs::canonicalize(project_root).map_or_else(|_| project_root.to_string(), |p| p.to_string_lossy().to_string());
    format!("w{:016x}", fnv1a64(canonical.as_bytes()))
}

pub fn write_turn_snapshot(
    project_root: &str,
    mut snapshot: TurnSnapshot,
) -> Result<String, String> {
    let tree = snapshot_worktree(project_root)?;
    snapshot.tree_hash = Some(tree.clone());

    let head = current_head(project_root);
    if snapshot.head_commit.is_none() {
        snapshot.head_commit.clone_from(&head);
    }
    if snapshot.base_commit.is_none() {
        snapshot.base_commit.clone_from(&head);
    }
    if snapshot.branch.is_none() {
        snapshot.branch = current_branch(project_root);
    }

    let message =
        serde_json::to_string_pretty(&snapshot).map_err(|e| format!("serialize turn: {e}"))?;
    let commit = create_commit(project_root, &tree, head.as_deref(), &message)?;
    let turn_ref = ref_for(&snapshot);
    git_in(project_root, &["update-ref", &turn_ref, &commit])?;
    git_in(
        project_root,
        &["update-ref", &head_ref_for(&snapshot), &commit],
    )?;
    Ok(commit)
}

pub fn read_turn_snapshot(project_root: &str, turn_id: &str) -> Option<TurnSnapshot> {
    list_turn_snapshots(project_root)
        .into_iter()
        .find(|snapshot| snapshot.id == turn_id)
}

pub fn list_turn_snapshots(project_root: &str) -> Vec<TurnSnapshot> {
    let refs = match git_in(
        project_root,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            REF_PREFIX,
        ],
    ) {
        Ok(out) => out,
        Err(_) => return Vec::new(),
    };

    let mut snapshots = Vec::new();
    for line in refs.lines() {
        let Some((refname, commit)) = line.split_once(' ') else {
            continue;
        };
        if refname.ends_with("/head") {
            continue;
        }
        if let Some(snapshot) = read_snapshot_from_commit(project_root, commit) {
            snapshots.push(snapshot);
        }
    }
    snapshots.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    snapshots
}

fn ref_for(snapshot: &TurnSnapshot) -> String {
    format!(
        "{}/{}/{}/{}/{}",
        REF_PREFIX,
        ref_segment(&snapshot.worktree_id),
        ref_segment(&snapshot.source),
        ref_segment(&snapshot.session_id),
        snapshot.id
    )
}

fn head_ref_for(snapshot: &TurnSnapshot) -> String {
    format!(
        "{}/{}/{}/{}/head",
        REF_PREFIX,
        ref_segment(&snapshot.worktree_id),
        ref_segment(&snapshot.source),
        ref_segment(&snapshot.session_id),
    )
}

fn ref_segment(raw: &str) -> String {
    let safe: String = raw
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => c,
            _ => '_',
        })
        .collect();
    let safe = safe.trim_matches('.').trim_matches('/').trim_matches('_');
    let safe = if safe.is_empty() { "x" } else { safe };
    format!("{safe}-{:016x}", fnv1a64(raw.as_bytes()))
}

fn snapshot_worktree(project_root: &str) -> Result<String, String> {
    let tmp = tempfile::tempdir().map_err(|e| format!("temp index: {e}"))?;
    let index_path = tmp.path().join("index");
    let index = index_path.to_string_lossy().to_string();
    let env = [("GIT_INDEX_FILE", index.as_str())];

    if current_head(project_root).is_some() {
        git_env_in(project_root, &["read-tree", "HEAD"], &env)?;
    } else {
        git_env_in(project_root, &["read-tree", "--empty"], &env)?;
    }

    git_env_in(project_root, &["add", "-A", "--", "."], &env)?;
    git_env_in(project_root, &["write-tree"], &env)
}

fn create_commit(
    project_root: &str,
    tree: &str,
    parent: Option<&str>,
    message: &str,
) -> Result<String, String> {
    let mut args = vec!["commit-tree".to_string(), tree.to_string()];
    if let Some(parent) = parent {
        args.push("-p".to_string());
        args.push(parent.to_string());
    }
    args.push("-F".to_string());
    args.push("-".to_string());
    git_stdin_in(project_root, &args, message)
}

fn read_snapshot_from_commit(project_root: &str, commit: &str) -> Option<TurnSnapshot> {
    let body = git_in(project_root, &["show", "-s", "--format=%B", commit]).ok()?;
    serde_json::from_str(body.trim()).ok()
}

fn current_head(project_root: &str) -> Option<String> {
    git_in(project_root, &["rev-parse", "--verify", "HEAD"]).ok()
}

fn current_branch(project_root: &str) -> Option<String> {
    git_in(project_root, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()
}

fn git_in(project_root: &str, args: &[&str]) -> Result<String, String> {
    git_command(project_root, args, &[], None)
}

fn git_env_in(project_root: &str, args: &[&str], env: &[(&str, &str)]) -> Result<String, String> {
    git_command(project_root, args, env, None)
}

fn git_stdin_in(project_root: &str, args: &[String], stdin_data: &str) -> Result<String, String> {
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    git_command(project_root, &borrowed, &[], Some(stdin_data))
}

fn git_command(
    project_root: &str,
    args: &[&str],
    env: &[(&str, &str)],
    stdin_data: Option<&str>,
) -> Result<String, String> {
    use std::io::Write;

    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut command = Command::new(git);
    command
        .args(args)
        .current_dir(project_root)
        .stdin(if stdin_data.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_QUARANTINE_PATH")
        .env("GIT_AUTHOR_NAME", "Oobo")
        .env("GIT_AUTHOR_EMAIL", "oobo@local")
        .env("GIT_COMMITTER_NAME", "Oobo")
        .env("GIT_COMMITTER_EMAIL", "oobo@local");

    for (k, v) in env {
        command.env(k, v);
    }

    let mut child = command.spawn().map_err(|e| format!("git: {e}"))?;
    if let Some(stdin_data) = stdin_data {
        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(stdin_data.as_bytes())
                .map_err(|e| format!("git stdin: {e}"))?;
        }
    }
    let output = child.wait_with_output().map_err(|e| format!("git: {e}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout)
            .replace('\r', "")
            .trim()
            .to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn init_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_str().unwrap();
        let init = Command::new("git").args(["init", repo]).output().unwrap();
        assert!(init.status.success());
        tmp
    }

    #[test]
    fn write_and_read_turn_snapshot_roundtrip() {
        let tmp = init_repo();
        let repo = tmp.path().to_str().unwrap();
        std::fs::write(Path::new(repo).join("hello.txt"), "hello\n").unwrap();

        let wt = worktree_id(repo);
        let mut snapshot = TurnSnapshot::new("p:test", &wt, "cursor", "session/one", 0);
        snapshot.memory.transcript_path = Some("/tmp/native-transcript.jsonl".to_string());
        snapshot.memory.transcript = Some(serde_json::json!({"messages": ["hi"]}));

        let commit = write_turn_snapshot(repo, snapshot.clone()).unwrap();
        assert!(!commit.is_empty());

        let read = read_turn_snapshot(repo, &snapshot.id).unwrap();
        assert_eq!(read.id, snapshot.id);
        assert_eq!(read.source, "cursor");
        assert_eq!(read.session_id, "session/one");
        assert!(read.tree_hash.is_some());
        assert_eq!(
            read.memory.transcript_path.as_deref(),
            Some("/tmp/native-transcript.jsonl")
        );

        let snapshots = list_turn_snapshots(repo);
        assert_eq!(snapshots.len(), 1);
    }

    #[test]
    fn snapshot_does_not_modify_user_index() {
        let tmp = init_repo();
        let repo = tmp.path().to_str().unwrap();
        std::fs::write(Path::new(repo).join("staged.txt"), "staged\n").unwrap();
        let add = Command::new("git")
            .args(["-C", repo, "add", "staged.txt"])
            .output()
            .unwrap();
        assert!(add.status.success());
        std::fs::write(Path::new(repo).join("untracked.txt"), "untracked\n").unwrap();

        let wt = worktree_id(repo);
        let snapshot = TurnSnapshot::new("p:test", &wt, "claude", "s1", 0);
        write_turn_snapshot(repo, snapshot).unwrap();

        let staged = Command::new("git")
            .args(["-C", repo, "diff", "--cached", "--name-only"])
            .output()
            .unwrap();
        assert!(staged.status.success());
        let stdout = String::from_utf8_lossy(&staged.stdout);
        assert_eq!(stdout.trim(), "staged.txt");
    }
}
