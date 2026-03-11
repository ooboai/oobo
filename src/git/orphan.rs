use std::fs;

use std::process::{Command, Stdio};

use crate::core::anchor::{Anchor, SessionLink, TransparencyMode};

const BRANCH: &str = "oobo/anchors/v1";

/// Write anchor metadata to the orphan branch for a given commit.
///
/// Layout on the branch:
/// ```text
/// c8/e12fa9b3d4.../
///   metadata.json       # Anchor-level metadata
///   1/metadata.json     # Per-session metadata
///   1/transcript.json   # (only when FullTransparency, redacted)
/// ```
///
/// Uses low-level git commands to update the orphan branch without
/// checking it out (so the user's working tree is never touched).
pub fn write_anchor(
    project_root: &str,
    anchor: &Anchor,
    session_links: &[SessionLink],
    transcripts: &[(String, String)],
) -> Result<(), String> {
    ensure_branch(project_root)?;

    let (prefix, rest) = shard_key(&anchor.commit_hash);
    let base_path = format!("{prefix}/{rest}");

    let anchor_json =
        serde_json::to_string_pretty(anchor).map_err(|e| format!("serialize anchor: {e}"))?;

    let mut entries: Vec<(String, String)> =
        vec![(format!("{base_path}/metadata.json"), anchor_json)];

    for (i, link) in session_links.iter().enumerate() {
        let link_json = serde_json::to_string_pretty(link)
            .map_err(|e| format!("serialize session link: {e}"))?;
        entries.push((
            format!("{base_path}/{}/{}", i + 1, "metadata.json"),
            link_json,
        ));

        if anchor.transparency_mode == TransparencyMode::On {
            if let Some((_, transcript_text)) =
                transcripts.iter().find(|(sid, _)| sid == &link.session_id)
            {
                let redacted = crate::redact::redact(transcript_text);
                let sanitized = strip_absolute_paths(&redacted, project_root);
                entries.push((format!("{base_path}/{}/transcript.json", i + 1), sanitized));
            }
        }
    }

    write_to_branch(project_root, &entries)?;

    Ok(())
}

/// Replace absolute paths containing the project root with repo-relative paths.
/// Also strips the user's home directory from any remaining absolute paths.
fn strip_absolute_paths(text: &str, project_root: &str) -> String {
    let mut result = text.to_string();

    // Strip project root (with and without trailing slash)
    let root_slash = if project_root.ends_with('/') {
        project_root.to_string()
    } else {
        format!("{project_root}/")
    };
    result = result.replace(&root_slash, "");
    result = result.replace(project_root, "");

    // Strip home directory from any other absolute paths
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        let home_slash = format!("{home_str}/");
        result = result.replace(&home_slash, "~/");
    }

    result
}

pub fn read_anchor(project_root: &str, commit_hash: &str) -> Option<Anchor> {
    let (prefix, rest) = shard_key(commit_hash);
    let path = format!("{prefix}/{rest}/metadata.json");

    let content = read_from_branch(project_root, &path)?;
    serde_json::from_str(&content).ok()
}

/// List all anchor commit hashes stored on the orphan branch.
/// Uses `git ls-tree` to enumerate directories without checking out.
pub fn list_anchor_hashes(project_root: &str) -> Vec<String> {
    let output = git_in(project_root, &["ls-tree", "-r", "--name-only", BRANCH]);
    let tree = match output {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };

    let mut hashes = std::collections::HashSet::new();
    for line in tree.lines() {
        // Lines look like: "c8/e12fa9b3d4…/metadata.json"
        // or "c8/e12fa9b3d4…/1/metadata.json" (session sub-dir)
        // We want the top-level metadata.json → reconstruct commit hash from first two path components.
        let parts: Vec<&str> = line.split('/').collect();
        if parts.len() == 3 && parts[2] == "metadata.json" {
            let hash = format!("{}{}", parts[0], parts[1]);
            hashes.insert(hash);
        }
    }
    hashes.into_iter().collect()
}

/// Read all session links for a given anchor from the orphan branch.
pub fn read_session_links(project_root: &str, commit_hash: &str) -> Vec<SessionLink> {
    let (prefix, rest) = shard_key(commit_hash);
    let base_path = format!("{prefix}/{rest}");

    let output = git_in(
        project_root,
        &["ls-tree", "--name-only", BRANCH, &format!("{base_path}/")],
    );
    let entries = match output {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };

    let mut links = Vec::new();
    for entry in entries.lines() {
        let name = entry.rsplit('/').next().unwrap_or(entry);
        if name.parse::<u32>().is_ok() {
            let path = format!("{base_path}/{name}/metadata.json");
            if let Some(content) = read_from_branch(project_root, &path) {
                if let Ok(link) = serde_json::from_str::<SessionLink>(&content) {
                    links.push(link);
                }
            }
        }
    }
    links
}

/// Hydrate the local SQLite database from anchors on the orphan branch.
/// Skips anchors that already exist in the DB. Returns the number of
/// new anchors imported.
pub fn hydrate_from_branch(project_root: &str, db: &crate::db::Db) -> Result<usize, String> {
    if !branch_exists(project_root) {
        return Ok(0);
    }

    let hashes = list_anchor_hashes(project_root);
    let mut imported = 0;

    for hash in &hashes {
        if db.anchor_exists(hash)? {
            continue;
        }

        let anchor = match read_anchor(project_root, hash) {
            Some(a) => a,
            None => continue,
        };

        let raw_json =
            serde_json::to_string(&anchor).map_err(|e| format!("serialize anchor: {e}"))?;
        db.insert_anchor(&anchor.commit_hash, &raw_json)?;

        let session_links = read_session_links(project_root, hash);
        for link in &session_links {
            let lt = match link.link_type {
                crate::core::anchor::LinkType::Explicit => "explicit",
                crate::core::anchor::LinkType::Inferred => "inferred",
            };
            db.insert_anchor_session(
                &anchor.commit_hash,
                &link.session_id,
                &link.agent,
                link.model.as_deref(),
                lt,
                link.files_touched.as_deref(),
            )?;
        }

        imported += 1;
    }

    Ok(imported)
}

/// Check if the orphan branch exists locally.
pub fn branch_exists(project_root: &str) -> bool {
    git_in(project_root, &["rev-parse", "--verify", BRANCH]).is_ok()
}

/// Check if the orphan branch exists on the remote.
pub fn remote_branch_exists(project_root: &str) -> bool {
    git_in(project_root, &["ls-remote", "--heads", "origin", BRANCH])
        .map(|out| !out.trim().is_empty())
        .unwrap_or(false)
}

const MAX_PUSH_ATTEMPTS: u32 = 5;

/// Push the orphan branch to origin. Retries with jittered backoff on
/// non-fast-forward errors (common when multiple agents push concurrently).
/// If all retries fail, records the failure for later retry via `retry_pending_pushes`.
pub fn push(project_root: &str) -> Result<(), String> {
    retry_pending_pushes(project_root);

    let mut last_err = String::new();
    for attempt in 0..MAX_PUSH_ATTEMPTS {
        match git_in(project_root, &["push", "--no-verify", "origin", BRANCH]) {
            Ok(_) => {
                clear_pending_push(project_root);
                return Ok(());
            }
            Err(e) if e.contains("non-fast-forward") || e.contains("rejected") => {
                last_err = e;
                if attempt < MAX_PUSH_ATTEMPTS - 1 {
                    let _ = git_in(project_root, &["fetch", "origin", BRANCH]);
                    let _ = git_in(
                        project_root,
                        &["rebase", &format!("origin/{BRANCH}"), BRANCH],
                    );
                    jitter_sleep(attempt);
                }
            }
            Err(e) => return Err(e),
        }
    }
    mark_pending_push(project_root);
    Err(format!(
        "push failed after {MAX_PUSH_ATTEMPTS} attempts: {last_err}"
    ))
}

/// Retry any previously failed pushes (called at the start of each push).
pub fn retry_pending_pushes(project_root: &str) {
    let path = pending_push_path(project_root);
    if !path.exists() {
        return;
    }
    let _ = git_in(project_root, &["fetch", "origin", BRANCH]);
    let _ = git_in(
        project_root,
        &["rebase", &format!("origin/{BRANCH}"), BRANCH],
    );
    if git_in(project_root, &["push", "--no-verify", "origin", BRANCH]).is_ok() {
        let _ = fs::remove_file(&path);
    }
}

fn pending_push_path(project_root: &str) -> std::path::PathBuf {
    std::path::Path::new(project_root).join(".git/oobo-push-pending")
}

fn mark_pending_push(project_root: &str) {
    let path = pending_push_path(project_root);
    let _ = fs::write(&path, chrono::Utc::now().to_rfc3339());
}

fn clear_pending_push(project_root: &str) {
    let path = pending_push_path(project_root);
    let _ = fs::remove_file(&path);
}

fn jitter_sleep(attempt: u32) {
    let base_ms = 100u64 * 2u64.pow(attempt);
    let jitter = rand_jitter_ms(base_ms / 2);
    std::thread::sleep(std::time::Duration::from_millis(base_ms + jitter));
}

/// Simple jitter without pulling in the `rand` crate.
fn rand_jitter_ms(max: u64) -> u64 {
    if max == 0 {
        return 0;
    }
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    seed % max
}

/// Fetch the orphan branch from origin (for first-use detection).
pub fn fetch(project_root: &str) -> Result<(), String> {
    git_in(
        project_root,
        &["fetch", "origin", &format!("{BRANCH}:{BRANCH}")],
    )?;
    Ok(())
}

// Internal helpers

/// Shard a commit hash into directory prefix + remainder.
/// `c8e12fa9b3d4...` → ("c8", "e12fa9b3d4...")
fn shard_key(hash: &str) -> (&str, &str) {
    if hash.len() >= 3 {
        (&hash[..2], &hash[2..])
    } else {
        (hash, "")
    }
}

/// Ensure the orphan branch exists. Creates it using plumbing commands only —
/// never touches the working tree or index, so uncommitted changes are safe.
fn ensure_branch(project_root: &str) -> Result<(), String> {
    if branch_exists(project_root) {
        return Ok(());
    }

    let readme_content = "# Oobo Anchors\n\nThis branch contains anchor metadata managed by oobo.\nDo not edit manually.\n";
    let blob = git_stdin_in(
        project_root,
        &["hash-object", "-w", "--stdin"],
        readme_content,
    )?;

    let tree_input = format!("100644 blob {blob}\tREADME.md\n");
    let tree = git_stdin_in(project_root, &["mktree"], &tree_input)?;

    let commit = git_stdin_in(
        project_root,
        &["commit-tree", &tree],
        "Initialize oobo/anchors/v1",
    )?;

    git_in(
        project_root,
        &["update-ref", &format!("refs/heads/{BRANCH}"), &commit],
    )?;

    Ok(())
}

/// Write entries to the orphan branch using `git hash-object` + `git update-index`
/// + `git write-tree` + `git commit-tree` to avoid ever checking out the branch.
fn write_to_branch(project_root: &str, entries: &[(String, String)]) -> Result<(), String> {
    let tree_hash = git_in(project_root, &["rev-parse", &format!("{BRANCH}^{{tree}}")])?;

    let env_key = "GIT_INDEX_FILE";
    let tmp_index = format!("{}/.git/oobo-index-tmp", project_root);

    git_env_in(
        project_root,
        &["read-tree", &tree_hash],
        &[(env_key, &tmp_index)],
    )?;

    for (path, content) in entries {
        let blob_hash = git_stdin_in(project_root, &["hash-object", "-w", "--stdin"], content)?;
        git_env_in(
            project_root,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                "100644",
                &blob_hash,
                path,
            ],
            &[(env_key, &tmp_index)],
        )?;
    }

    let new_tree = git_env_in(project_root, &["write-tree"], &[(env_key, &tmp_index)])?;

    let _ = fs::remove_file(&tmp_index);

    let parent = git_in(project_root, &["rev-parse", BRANCH])?;
    let new_commit = git_stdin_in(
        project_root,
        &["commit-tree", &new_tree, "-p", &parent],
        &format!(
            "oobo: add anchor for {}",
            entries.first().map(|(p, _)| p.as_str()).unwrap_or("?")
        ),
    )?;

    git_in(
        project_root,
        &["update-ref", &format!("refs/heads/{BRANCH}"), &new_commit],
    )?;

    Ok(())
}

fn read_from_branch(project_root: &str, path: &str) -> Option<String> {
    git_in(project_root, &["show", &format!("{BRANCH}:{path}")]).ok()
}

/// Run a git command in a specific directory and capture stdout.
/// Uses the real git binary and clears hook-related env vars so this
/// works correctly when called from inside a git hook.
fn git_in(project_root: &str, args: &[&str]) -> Result<String, String> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let output = Command::new(git)
        .args(args)
        .current_dir(project_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_QUARANTINE_PATH")
        .output()
        .map_err(|e| format!("git: {e}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Run a git command with extra environment variables.
fn git_env_in(project_root: &str, args: &[&str], env: &[(&str, &str)]) -> Result<String, String> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut cmd = Command::new(git);
    cmd.args(args)
        .current_dir(project_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_QUARANTINE_PATH");

    for (k, v) in env {
        cmd.env(k, v);
    }

    let output = cmd.output().map_err(|e| format!("git: {e}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Run a git command with stdin data.
fn git_stdin_in(project_root: &str, args: &[&str], stdin_data: &str) -> Result<String, String> {
    use std::io::Write;

    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut child = Command::new(git)
        .args(args)
        .current_dir(project_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_QUARANTINE_PATH")
        .spawn()
        .map_err(|e| format!("git: {e}"))?;

    if let Some(ref mut stdin) = child.stdin {
        stdin
            .write_all(stdin_data.as_bytes())
            .map_err(|e| format!("write stdin: {e}"))?;
    }

    let output = child.wait_with_output().map_err(|e| format!("git: {e}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::anchor::{AuthorType, Contributor, ContributorRole, LinkType};

    #[test]
    fn test_shard_key() {
        let (prefix, rest) = shard_key("c8e12fa9b3d4abcdef1234567890");
        assert_eq!(prefix, "c8");
        assert_eq!(rest, "e12fa9b3d4abcdef1234567890");
    }

    #[test]
    fn test_shard_key_short() {
        let (prefix, rest) = shard_key("ab");
        assert_eq!(prefix, "ab");
        assert_eq!(rest, "");
    }

    #[test]
    fn test_shard_key_exactly_3() {
        let (prefix, rest) = shard_key("abc");
        assert_eq!(prefix, "ab");
        assert_eq!(rest, "c");
    }

    #[test]
    fn test_shard_key_single_char() {
        let (prefix, rest) = shard_key("a");
        assert_eq!(prefix, "a");
        assert_eq!(rest, "");
    }

    #[test]
    fn test_shard_key_empty() {
        let (prefix, rest) = shard_key("");
        assert_eq!(prefix, "");
        assert_eq!(rest, "");
    }

    fn make_test_anchor(commit_hash: &str) -> Anchor {
        Anchor {
            oobo_version: "0.1.0".into(),
            commit_hash: commit_hash.into(),
            branch: "main".into(),
            author: "Test User <test@test.com>".into(),
            author_type: AuthorType::Assisted,
            contributors: vec![
                Contributor {
                    name: "Test User".into(),
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
            message: "feat: add widget support".into(),
            files_changed: vec!["src/widget.rs".into(), "src/lib.rs".into()],
            added: 42,
            deleted: 5,
            file_changes: vec![
                crate::core::anchor::FileChange {
                    path: "src/widget.rs".into(),
                    added: 40,
                    deleted: 0,
                    attribution: Some(crate::core::anchor::FileAttribution::Ai),
                    agent: Some("cursor".into()),
                },
                crate::core::anchor::FileChange {
                    path: "src/lib.rs".into(),
                    added: 2,
                    deleted: 5,
                    attribution: Some(crate::core::anchor::FileAttribution::Human),
                    agent: None,
                },
            ],
            ai_added: 40,
            ai_deleted: 0,
            human_added: 2,
            human_deleted: 5,
            ai_percentage: Some(85.1),
            session_ids: vec!["sess-abc".into()],
            summary: Some("Added widget module".into()),
            intent: None,
            reasoning: None,
            transparency_mode: TransparencyMode::Off,
        }
    }

    #[test]
    fn test_write_and_read_anchor_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_str().unwrap();

        let init = std::process::Command::new("git")
            .args(["init", repo])
            .output();
        if init.is_err() || !init.unwrap().status.success() {
            eprintln!("skipping test: git not available");
            return;
        }

        // Need an initial commit so git plumbing works
        let _ = std::process::Command::new("git")
            .args(["-C", repo, "commit", "--allow-empty", "-m", "init"])
            .output();

        let commit_hash = "c8e12fa9b3d4abcdef1234567890abcdef123456";
        let anchor = make_test_anchor(commit_hash);

        let session_link = SessionLink {
            session_id: "sess-abc".into(),
            agent: "cursor".into(),
            model: Some("claude-sonnet-4-20250514".into()),
            link_type: LinkType::Explicit,
            input_tokens: Some(15000),
            output_tokens: Some(8000),
            cache_read_tokens: None,
            cache_creation_tokens: None,
            duration_secs: Some(120),
            tool_calls: Some(5),
            files_touched: Some(vec!["src/widget.rs".into()]),
            is_subagent: false,
        };

        let result = write_anchor(repo, &anchor, &[session_link], &[]);
        assert!(result.is_ok(), "write_anchor failed: {:?}", result.err());

        assert!(
            branch_exists(repo),
            "orphan branch should exist after write"
        );

        let read_back = read_anchor(repo, commit_hash);
        assert!(
            read_back.is_some(),
            "should be able to read back the anchor"
        );

        let restored = read_back.unwrap();
        assert_eq!(restored.commit_hash, commit_hash);
        assert_eq!(restored.branch, "main");
        assert_eq!(restored.author, "Test User <test@test.com>");
        assert_eq!(restored.author_type, AuthorType::Assisted);
        assert_eq!(restored.added, 42);
        assert_eq!(restored.deleted, 5);
        assert_eq!(restored.files_changed.len(), 2);
        assert_eq!(restored.session_ids, vec!["sess-abc"]);
        assert_eq!(restored.summary.as_deref(), Some("Added widget module"));
        assert_eq!(restored.transparency_mode, TransparencyMode::Off);
    }

    #[test]
    fn test_read_anchor_nonexistent_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_str().unwrap();

        let init = std::process::Command::new("git")
            .args(["init", repo])
            .output();
        if init.is_err() || !init.unwrap().status.success() {
            return;
        }

        let result = read_anchor(repo, "deadbeefdeadbeef");
        assert!(result.is_none());
    }

    #[test]
    fn test_branch_exists_false_in_fresh_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_str().unwrap();

        let init = std::process::Command::new("git")
            .args(["init", repo])
            .output();
        if init.is_err() || !init.unwrap().status.success() {
            return;
        }

        assert!(!branch_exists(repo));
    }

    #[test]
    fn test_strip_absolute_paths_project_root() {
        let root = "/Users/teddy/dev/projects/trender";
        let input = r#"{"tool_call":{"params":{"path":"/Users/teddy/dev/projects/trender/backdate.sh"}}}"#;
        let result = strip_absolute_paths(input, root);
        assert_eq!(
            result,
            r#"{"tool_call":{"params":{"path":"backdate.sh"}}}"#
        );
    }

    #[test]
    fn test_strip_absolute_paths_nested() {
        let root = "/Users/teddy/dev/projects/myapp";
        let input = r#"{"path":"/Users/teddy/dev/projects/myapp/src/lib.rs"}"#;
        let result = strip_absolute_paths(input, root);
        assert_eq!(result, r#"{"path":"src/lib.rs"}"#);
    }

    #[test]
    fn test_strip_absolute_paths_home_fallback() {
        let root = "/Users/teddy/dev/projects/myapp";
        let home = dirs::home_dir().unwrap();
        let home_str = home.to_string_lossy();
        let input = format!(r#"{{"path":"{home_str}/.config/something"}}"#);
        let result = strip_absolute_paths(&input, root);
        assert_eq!(result, r#"{"path":"~/.config/something"}"#);
    }

    #[test]
    fn test_strip_absolute_paths_no_change_for_relative() {
        let root = "/Users/teddy/dev/projects/myapp";
        let input = r#"{"path":"src/main.rs"}"#;
        let result = strip_absolute_paths(input, root);
        assert_eq!(result, r#"{"path":"src/main.rs"}"#);
    }
}
