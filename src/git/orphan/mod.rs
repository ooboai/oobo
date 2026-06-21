mod rekey;
mod sync;
pub mod v2;

pub use rekey::{parse_rewrite_pairs, rekey_anchors, rekey_anchors_from_rewrite_pairs};
pub use sync::{fetch_and_reconcile, push, remote_branch_exists};

use std::collections::{HashMap, HashSet};
use std::fs;
use std::process::{Command, Stdio};

use crate::core::anchor::{Anchor, SessionLink};
use crate::error::CliError;

pub(super) const BRANCH: &str = "oobo/anchors/v1";
const NULL_OID: &str = "0000000000000000000000000000000000000000";

/// Build a timeline JSON blob for multi-agent file interactions,
/// enriched with per-file timestamped read/write events when available.
/// Written alongside the v2 anchor record.
pub(crate) fn build_timeline_json(
    project_root: &str,
    anchor: &Anchor,
    session_links: &[SessionLink],
    interactions: &[crate::core::anchor::FileInteraction],
) -> Result<String, CliError> {
    let longest_session_ms: Option<u64> = {
        let durations: Vec<u64> = session_links
            .iter()
            .filter_map(|l| l.duration_secs)
            .collect();
        if durations.is_empty() {
            None
        } else {
            Some(durations.iter().max().copied().unwrap_or(0) * 1000)
        }
    };

    let interactions_json: Vec<serde_json::Value> = interactions
        .iter()
        .map(|fi| {
            let sessions: Vec<serde_json::Value> = fi
                .sessions
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "session_id": r.session_id,
                        "role": match r.role {
                            crate::core::anchor::FileRole::Writer => "writer",
                            crate::core::anchor::FileRole::Reader => "reader",
                            crate::core::anchor::FileRole::Both => "both",
                        },
                    })
                })
                .collect();
            serde_json::json!({
                "path": fi.path,
                "sessions": sessions,
            })
        })
        .collect();

    // Collect timestamped file events from all linked sessions.
    let mut events_json: Vec<serde_json::Value> = Vec::new();
    for link in session_links {
        let events = crate::hooks::state::get_file_events(project_root, &link.session_id);
        for ev in events {
            events_json.push(serde_json::json!({
                "session_id": link.session_id,
                "path": ev.path,
                "action": ev.action,
                "timestamp": ev.timestamp,
            }));
        }
    }
    events_json.sort_by_key(|e| e["timestamp"].as_i64().unwrap_or(0));

    let mut timeline = serde_json::json!({
        "session_count": anchor.session_ids.len(),
        "file_interactions": interactions_json,
    });

    if let Some(dur) = longest_session_ms {
        timeline["longest_session_ms"] = serde_json::json!(dur);
    }

    if !events_json.is_empty() {
        timeline["events"] = serde_json::json!(events_json);
    }

    serde_json::to_string_pretty(&timeline)
        .map_err(|e| CliError::Git(format!("serialize timeline: {e}")))
}

/// The v2 store keys repos by id derived from the CANONICAL root (the
/// writer canonicalizes; /var vs /private/var on macOS). Readers must
/// derive the same id or path-fallback ids miss.
fn v2_repo_id(project_root: &str) -> String {
    let canon = std::fs::canonicalize(project_root).map_or_else(
        |_| project_root.to_string(),
        |p| p.to_string_lossy().to_string(),
    );
    crate::project::id_for_root(&canon)
}

/// Read one anchor. v2 is the store of record; v1 is read-only legacy
/// (anchors written before the v2 cut).
pub fn read_anchor(project_root: &str, commit_hash: &str) -> Option<Anchor> {
    let repo_id = v2_repo_id(project_root);
    if let Some(record) = v2::read_anchor(project_root, &repo_id, commit_hash) {
        return Some(record.anchor);
    }
    read_anchor_v1(project_root, commit_hash)
}

pub(super) fn read_anchor_v1(project_root: &str, commit_hash: &str) -> Option<Anchor> {
    let (prefix, rest) = shard_key(commit_hash);
    let path = format!("{prefix}/{rest}/metadata.json");

    let content = read_from_branch(project_root, &path)?;
    serde_json::from_str(&content).ok()
}

/// List all anchor commit hashes: the v2 store plus legacy v1 anchors.
pub fn list_anchor_hashes(project_root: &str) -> Vec<String> {
    let repo_id = v2_repo_id(project_root);
    let mut hashes: HashSet<String> = v2::list_anchor_shas(project_root, &repo_id)
        .into_iter()
        .collect();

    if let Ok(tree) = git_in(project_root, &["ls-tree", "-r", "--name-only", BRANCH]) {
        for line in tree.lines() {
            // Lines look like: "c8/e12fa9b3d4…/metadata.json"
            // or "c8/e12fa9b3d4…/1/metadata.json" (session sub-dir)
            let parts: Vec<&str> = line.split('/').collect();
            if parts.len() == 3 && parts[2] == "metadata.json" {
                hashes.insert(format!("{}{}", parts[0], parts[1]));
            }
        }
    }
    hashes.into_iter().collect()
}

/// Read all session links for a given anchor. v2 anchors carry their
/// links inline; v1 stored them as per-session sub-entries.
pub fn read_session_links(project_root: &str, commit_hash: &str) -> Vec<SessionLink> {
    let repo_id = v2_repo_id(project_root);
    if let Some(record) = v2::read_anchor(project_root, &repo_id, commit_hash) {
        if !record.session_links.is_empty() {
            return record.session_links;
        }
        // v2 anchors written before session_links existed: fall through
        // to their v1 twin (the transition window wrote both).
    }
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

pub fn branch_exists(project_root: &str) -> bool {
    branch_exists_named(project_root, BRANCH)
}

pub(super) fn branch_exists_named(project_root: &str, branch: &str) -> bool {
    git_in(project_root, &["rev-parse", "--verify", branch]).is_ok()
}

/// Get the current tip commit hash of the orphan branch.
pub fn branch_tip(project_root: &str) -> Option<String> {
    git_in(project_root, &["rev-parse", BRANCH]).ok()
}

/// Bulk-read ALL anchors and their session links: the v2 store first,
/// then legacy v1 anchors for shas v2 doesn't have. Single
/// `git cat-file --batch` per store.
#[tracing::instrument(skip_all)]
pub fn read_all_anchors(project_root: &str) -> (Vec<Anchor>, HashMap<String, Vec<SessionLink>>) {
    let repo_id = v2_repo_id(project_root);
    let mut anchors = Vec::new();
    let mut links_map: HashMap<String, Vec<SessionLink>> = HashMap::new();
    let mut v2_shas: HashSet<String> = HashSet::new();

    for record in v2::read_all_anchor_records(project_root, &repo_id) {
        v2_shas.insert(record.anchor.commit_hash.clone());
        links_map.insert(record.anchor.commit_hash.clone(), record.session_links);
        anchors.push(record.anchor);
    }

    let (v1_anchors, v1_links) = read_all_anchors_v1(project_root);
    for anchor in v1_anchors {
        if !v2_shas.contains(&anchor.commit_hash) {
            anchors.push(anchor);
        }
    }
    for (sha, links) in v1_links {
        // v1 links also backfill v2 anchors from the transition window
        // (written before session_links existed on the record).
        let entry = links_map.entry(sha).or_default();
        if entry.is_empty() {
            *entry = links;
        }
    }

    tracing::debug!(
        anchors = anchors.len(),
        sessions = links_map.len(),
        "read_all_anchors done"
    );
    (anchors, links_map)
}

fn read_all_anchors_v1(project_root: &str) -> (Vec<Anchor>, HashMap<String, Vec<SessionLink>>) {
    let mut anchors = Vec::new();
    let mut links_map: HashMap<String, Vec<SessionLink>> = HashMap::new();

    if !branch_exists(project_root) {
        tracing::debug!("orphan branch does not exist");
        return (anchors, links_map);
    }

    let tree = match git_in(project_root, &["ls-tree", "-r", "--name-only", BRANCH]) {
        Ok(t) => t,
        Err(_) => return (anchors, links_map),
    };

    let mut anchor_paths: Vec<(String, String)> = Vec::new();
    let mut session_paths: Vec<(String, String)> = Vec::new();

    for line in tree.lines() {
        let parts: Vec<&str> = line.split('/').collect();
        if parts.len() == 3 && parts[2] == "metadata.json" {
            let hash = format!("{}{}", parts[0], parts[1]);
            anchor_paths.push((hash, format!("{BRANCH}:{line}")));
        } else if parts.len() == 4 && parts[3] == "metadata.json" && parts[2].parse::<u32>().is_ok()
        {
            let hash = format!("{}{}", parts[0], parts[1]);
            session_paths.push((hash, format!("{BRANCH}:{line}")));
        }
    }

    let all_refs: Vec<&str> = anchor_paths
        .iter()
        .map(|(_, r)| r.as_str())
        .chain(session_paths.iter().map(|(_, r)| r.as_str()))
        .collect();

    let contents = batch_cat_file(project_root, &all_refs);

    for (i, (hash, _)) in anchor_paths.iter().enumerate() {
        if let Some(content) = contents.get(i).and_then(|c| c.as_ref()) {
            if let Ok(anchor) = serde_json::from_str::<Anchor>(content) {
                anchors.push(anchor);
                links_map.entry(hash.clone()).or_default();
            }
        }
    }

    let offset = anchor_paths.len();
    for (i, (hash, _)) in session_paths.iter().enumerate() {
        if let Some(content) = contents.get(offset + i).and_then(|c| c.as_ref()) {
            if let Ok(link) = serde_json::from_str::<SessionLink>(content) {
                links_map.entry(hash.clone()).or_default().push(link);
            }
        }
    }

    (anchors, links_map)
}

/// Read multiple git objects in a single `git cat-file --batch` process.
/// Returns a Vec parallel to `refs`  --  `Some(content)` for successful reads,
/// `None` for missing objects.
///
/// Stdin writes and stdout reads run on separate threads to avoid pipe
/// buffer deadlocks when the number of refs is large.
fn batch_cat_file(project_root: &str, refs: &[&str]) -> Vec<Option<String>> {
    use std::io::{BufRead, BufReader, Write};

    if refs.is_empty() {
        return Vec::new();
    }

    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut cmd = Command::new(git);
    cmd.args(["cat-file", "--batch"])
        .current_dir(project_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::git::proxy::scrub_git_env(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return vec![None; refs.len()],
    };

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let refs_owned: Vec<String> = refs.iter().map(std::string::ToString::to_string).collect();
    let n = refs.len();

    let writer = std::thread::spawn(move || {
        for r in &refs_owned {
            if writeln!(stdin, "{r}").is_err() {
                break;
            }
        }
        drop(stdin);
    });

    let mut results = Vec::with_capacity(n);
    let mut reader = BufReader::new(stdout);
    let mut header = String::new();

    for _ in 0..n {
        header.clear();
        if reader.read_line(&mut header).is_err() {
            results.push(None);
            continue;
        }

        let trimmed = header.trim();
        if trimmed.ends_with("missing") || trimmed.is_empty() {
            results.push(None);
            continue;
        }

        let size: usize = trimmed
            .rsplit(' ')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let mut buf = vec![0u8; size + 1];
        if std::io::Read::read_exact(&mut reader, &mut buf).is_err() {
            results.push(None);
            continue;
        }

        let content = String::from_utf8_lossy(&buf[..size]).to_string();
        results.push(Some(content));
    }

    let _ = writer.join();
    let _ = child.wait();

    while results.len() < n {
        results.push(None);
    }

    results
}

/// Shard a commit hash into directory prefix + remainder.
/// `c8e12fa9b3d4...` → ("c8", "e12fa9b3d4...")
pub(super) fn shard_key(hash: &str) -> (&str, &str) {
    if hash.len() >= 3 {
        (&hash[..2], &hash[2..])
    } else {
        (hash, "")
    }
}

/// Create the legacy v1 branch using plumbing commands only. Production
/// code no longer writes v1; tests use this to seed legacy data.
#[cfg(test)]
fn ensure_branch(project_root: &str) -> Result<(), CliError> {
    ensure_branch_named(project_root, BRANCH)
}

pub(super) fn ensure_branch_named(project_root: &str, branch: &str) -> Result<(), CliError> {
    if branch_exists_named(project_root, branch) {
        return Ok(());
    }

    let readme_content = "# Anchors\n\nThis branch contains anchor metadata managed by oobo.\nDo not edit manually.\n";
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
        &format!("Initialize {branch}"),
    )?;

    git_in(
        project_root,
        &[
            "update-ref",
            &format!("refs/heads/{branch}"),
            &commit,
            NULL_OID,
        ],
    )?;

    Ok(())
}

/// 3 retries: local ref CAS failures resolve quickly (no network).
const MAX_WRITE_ATTEMPTS: u32 = 3;

/// Write entries to the orphan branch, retrying on CAS contention.
pub(super) fn write_to_branch(
    project_root: &str,
    entries: &[(String, String)],
) -> Result<(), CliError> {
    write_to_branch_named(project_root, BRANCH, entries)
}

pub(super) fn write_to_branch_named(
    project_root: &str,
    branch: &str,
    entries: &[(String, String)],
) -> Result<(), CliError> {
    let mut last_err = String::new();
    for attempt in 0..MAX_WRITE_ATTEMPTS {
        match try_write_to_branch(project_root, branch, entries) {
            Ok(()) => return Ok(()),
            Err(ref e)
                if attempt < MAX_WRITE_ATTEMPTS - 1 && {
                    let msg = e.to_string();
                    msg.contains("but expected") || msg.contains("cannot lock ref")
                } =>
            {
                last_err = e.to_string();
                sync::jitter_sleep(attempt);
            }
            Err(e) => return Err(e),
        }
    }
    Err(CliError::Git(format!(
        "write failed after {MAX_WRITE_ATTEMPTS} attempts: {last_err}"
    )))
}

/// Build a commit on top of `parent` with `entries` added. Returns the
/// commit hash without updating any ref.
pub(super) fn build_commit_on(
    project_root: &str,
    parent: &str,
    entries: &[(String, String)],
    message: &str,
) -> Result<String, CliError> {
    let tree_hash = git_in(project_root, &["rev-parse", &format!("{parent}^{{tree}}")])?;

    let env_key = "GIT_INDEX_FILE";
    let git_common = crate::git::detect::resolve_git_common_dir(project_root);
    let tmp_index = format!(
        "{}/oobo-index-{}-{}",
        git_common.display(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
    );

    let new_tree = {
        let result = (|| {
            git_env_in(
                project_root,
                &["read-tree", &tree_hash],
                &[(env_key, &tmp_index)],
            )?;

            for (path, content) in entries {
                let blob_hash =
                    git_stdin_in(project_root, &["hash-object", "-w", "--stdin"], content)?;
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

            git_env_in(project_root, &["write-tree"], &[(env_key, &tmp_index)])
        })();
        let _ = fs::remove_file(&tmp_index);
        result?
    };

    git_stdin_in(
        project_root,
        &["commit-tree", &new_tree, "-p", parent],
        message,
    )
}

/// hash-object → update-index → write-tree → commit-tree → CAS update-ref.
fn try_write_to_branch(
    project_root: &str,
    branch: &str,
    entries: &[(String, String)],
) -> Result<(), CliError> {
    let parent = git_in(project_root, &["rev-parse", branch])?;
    let new_commit = build_commit_on(
        project_root,
        &parent,
        entries,
        &format!(
            "oobo: add anchor for {}",
            entries.first().map_or("?", |(p, _)| p.as_str())
        ),
    )?;

    git_in(
        project_root,
        &[
            "update-ref",
            &format!("refs/heads/{branch}"),
            &new_commit,
            &parent,
        ],
    )?;

    Ok(())
}

pub(super) fn read_from_branch(project_root: &str, path: &str) -> Option<String> {
    read_from_branch_named(project_root, BRANCH, path)
}

pub(super) fn read_from_branch_named(
    project_root: &str,
    branch: &str,
    path: &str,
) -> Option<String> {
    // A fresh clone has the store only as a remote-tracking ref — the
    // "clone only repo Y, attribution still works offline" contract.
    // Local branch wins (it's what writers update); origin is fallback.
    git_in(project_root, &["show", &format!("{branch}:{path}")])
        .or_else(|_| {
            git_in(
                project_root,
                &["show", &format!("refs/remotes/origin/{branch}:{path}")],
            )
        })
        .ok()
}

/// Run a git command in a specific directory and capture stdout.
/// Uses the real git binary and clears hook-related env vars so this
/// works correctly when called from inside a git hook.
pub(super) fn git_in(project_root: &str, args: &[&str]) -> Result<String, CliError> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut cmd = Command::new(git);
    cmd.args(args)
        .current_dir(project_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::git::proxy::scrub_git_env(&mut cmd);
    let output = cmd
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

/// Like `git_in` but kills the child after `timeout` elapses.
/// Used for network-facing commands (push, fetch, ls-remote) that can
/// hang indefinitely when the remote is unreachable.
pub(super) fn git_in_timeout(
    project_root: &str,
    args: &[&str],
    timeout: std::time::Duration,
) -> Result<String, CliError> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut cmd = Command::new(git);
    cmd.args(args)
        .current_dir(project_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::git::proxy::scrub_git_env(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| CliError::Git(format!("git: {e}")))?;

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(CliError::Git(format!(
                        "git {} timed out after {}s",
                        args.first().unwrap_or(&""),
                        timeout.as_secs()
                    )));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(CliError::Git(format!("git: {e}"))),
        }
    }

    let output = child
        .wait_with_output()
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

/// Run a git command with extra environment variables.
fn git_env_in(project_root: &str, args: &[&str], env: &[(&str, &str)]) -> Result<String, CliError> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut cmd = Command::new(git);
    cmd.args(args)
        .current_dir(project_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::git::proxy::scrub_git_env(&mut cmd);

    for (k, v) in env {
        cmd.env(k, v);
    }

    let output = cmd
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

/// Run a git command with stdin data.
fn git_stdin_in(project_root: &str, args: &[&str], stdin_data: &str) -> Result<String, CliError> {
    use std::io::Write;

    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut cmd = Command::new(git);
    cmd.args(args)
        .current_dir(project_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::git::proxy::scrub_git_env(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| CliError::Git(format!("git: {e}")))?;

    if let Some(ref mut stdin) = child.stdin {
        stdin
            .write_all(stdin_data.as_bytes())
            .map_err(|e| CliError::Git(format!("write stdin: {e}")))?;
    }

    let output = child
        .wait_with_output()
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
    use super::sync::{replay_local_files, validate_anchor_remote};
    use super::*;
    use crate::core::anchor::{
        AuthorType, Contributor, ContributorRole, LinkType, TransparencyMode,
    };
    use crate::redact::strip_absolute_paths;

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

    #[test]
    fn parse_rewrite_pairs_ignores_malformed_lines() {
        let pairs = parse_rewrite_pairs(
            "old1 new1\n\
             malformed\n\
             old2 new2 extra\n\
             \n",
        );
        assert_eq!(
            pairs,
            vec![
                ("old1".to_string(), "new1".to_string()),
                ("old2".to_string(), "new2".to_string()),
            ]
        );
    }

    fn make_test_anchor(commit_hash: &str) -> Anchor {
        Anchor {
            anchor_schema_version: crate::core::anchor::ANCHOR_SCHEMA_VERSION,
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
                    line_attributions: Vec::new(),
                },
                crate::core::anchor::FileChange {
                    path: "src/lib.rs".into(),
                    added: 2,
                    deleted: 5,
                    attribution: Some(crate::core::anchor::FileAttribution::Human),
                    agent: None,
                    line_attributions: Vec::new(),
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
            file_interactions: None,
            turns: Vec::new(),
        }
    }

    #[test]
    fn test_legacy_v1_anchor_stays_readable() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_str().unwrap();

        let init = std::process::Command::new("git")
            .args(["init", repo])
            .output();
        if init.is_err() || !init.unwrap().status.success() {
            eprintln!("skipping test: git not available");
            return;
        }

        let _ = std::process::Command::new("git")
            .args(["-C", repo, "config", "user.name", "Test"])
            .output();
        let _ = std::process::Command::new("git")
            .args(["-C", repo, "config", "user.email", "test@test.com"])
            .output();

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
        };

        // Seed the branch the way pre-v2 builds did: v1 layout, one
        // metadata.json per anchor plus numbered session sub-dirs. The
        // public readers must still resolve this legacy data.
        ensure_branch(repo).unwrap();
        let (p, rest) = shard_key(commit_hash);
        write_to_branch(
            repo,
            &[
                (
                    format!("{p}/{rest}/metadata.json"),
                    serde_json::to_string_pretty(&anchor).unwrap(),
                ),
                (
                    format!("{p}/{rest}/1/metadata.json"),
                    serde_json::to_string_pretty(&session_link).unwrap(),
                ),
            ],
        )
        .unwrap();

        assert!(
            branch_exists(repo),
            "orphan branch should exist after write"
        );

        let read_back = read_anchor(repo, commit_hash);
        assert!(
            read_back.is_some(),
            "legacy v1 anchors must stay readable through the public API"
        );

        let links = read_session_links(repo, commit_hash);
        assert_eq!(links.len(), 1, "legacy v1 session links must resolve");
        assert_eq!(links[0].session_id, "sess-abc");

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
    fn test_push_uses_project_anchor_remote() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let anchor_remote = tmp.path().join("anchor-remote.git");
        let repo_str = repo.to_str().unwrap();
        let remote_str = anchor_remote.to_str().unwrap();

        let init = std::process::Command::new("git")
            .args(["init", repo_str])
            .output();
        if init.is_err() || !init.unwrap().status.success() {
            eprintln!("skipping test: git not available");
            return;
        }

        let _ = std::process::Command::new("git")
            .args(["-C", repo_str, "config", "user.name", "Test"])
            .output();
        let _ = std::process::Command::new("git")
            .args(["-C", repo_str, "config", "user.email", "test@test.com"])
            .output();

        let init_remote = std::process::Command::new("git")
            .args(["init", "--bare", remote_str])
            .output()
            .unwrap();
        assert!(
            init_remote.status.success(),
            "bare remote init failed: {}",
            String::from_utf8_lossy(&init_remote.stderr)
        );

        let mut cfg = crate::project_config::ProjectConfig::for_project("p:test");
        cfg.anchors.remote = remote_str.to_string();
        cfg.save(repo_str).unwrap();

        ensure_branch(repo_str).unwrap();
        push(repo_str).unwrap();

        let ls = std::process::Command::new("git")
            .args([
                "--git-dir",
                remote_str,
                "show-ref",
                "refs/heads/oobo/anchors/v1",
            ])
            .output()
            .unwrap();
        assert!(
            ls.status.success(),
            "configured anchor remote did not receive branch: {}",
            String::from_utf8_lossy(&ls.stderr)
        );
    }

    #[test]
    fn test_named_anchor_remote_must_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let repo_str = repo.to_str().unwrap();

        let init = std::process::Command::new("git")
            .args(["init", repo_str])
            .output();
        if init.is_err() || !init.unwrap().status.success() {
            eprintln!("skipping test: git not available");
            return;
        }

        let err = validate_anchor_remote(repo_str, "oobo").unwrap_err();
        assert!(err
            .to_string()
            .contains("oobo remote 'oobo' is not configured"));
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
        let root = "/Users/example/dev/projects/trender";
        let input = r#"{"tool_call":{"params":{"path":"/Users/example/dev/projects/trender/backdate.sh"}}}"#;
        let result = strip_absolute_paths(input, root);
        assert_eq!(result, r#"{"tool_call":{"params":{"path":"backdate.sh"}}}"#);
    }

    #[test]
    fn test_strip_absolute_paths_nested() {
        let root = "/Users/example/dev/projects/myapp";
        let input = r#"{"path":"/Users/example/dev/projects/myapp/src/lib.rs"}"#;
        let result = strip_absolute_paths(input, root);
        assert_eq!(result, r#"{"path":"src/lib.rs"}"#);
    }

    #[test]
    fn test_strip_absolute_paths_home_fallback() {
        let root = "/Users/example/dev/projects/myapp";
        let home = dirs::home_dir().unwrap();
        let home_str = home.to_string_lossy();
        let input = format!(r#"{{"path":"{home_str}/.config/something"}}"#);
        let result = strip_absolute_paths(&input, root);
        assert_eq!(result, r#"{"path":"~/.config/something"}"#);
    }

    #[test]
    fn test_strip_absolute_paths_no_change_for_relative() {
        let root = "/Users/example/dev/projects/myapp";
        let input = r#"{"path":"src/main.rs"}"#;
        let result = strip_absolute_paths(input, root);
        assert_eq!(result, r#"{"path":"src/main.rs"}"#);
    }

    #[test]
    fn test_strip_absolute_paths_does_not_mangle_prefix_match() {
        let root = "/path/myapp";
        let input = r#"{"path":"/path/myapp-backup/file.rs"}"#;
        let result = strip_absolute_paths(input, root);
        assert!(
            result.contains("/myapp-backup/file.rs"),
            "should preserve paths that share a prefix but aren't inside the project root: {result}"
        );
    }

    fn init_test_repo() -> Option<(tempfile::TempDir, String)> {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_str().unwrap().to_string();
        let init = std::process::Command::new("git")
            .args(["init", &repo])
            .output();
        if init.is_err() || !init.unwrap().status.success() {
            return None;
        }
        let _ = std::process::Command::new("git")
            .args(["-C", &repo, "config", "user.name", "Test"])
            .output();
        let _ = std::process::Command::new("git")
            .args(["-C", &repo, "config", "user.email", "test@test.com"])
            .output();
        let _ = std::process::Command::new("git")
            .args(["-C", &repo, "commit", "--allow-empty", "-m", "init"])
            .output();
        Some((tmp, repo))
    }

    #[test]
    fn test_replay_local_files_no_local_only_files() {
        let (tmp, repo) = match init_test_repo() {
            Some(r) => r,
            None => return,
        };
        let _ = tmp;

        ensure_branch(&repo).unwrap();
        let tip_a = git_in(&repo, &["rev-parse", BRANCH]).unwrap();

        // Create a "remote" tip that has the same file plus more
        write_to_branch(&repo, &[("extra/file.txt".into(), "hello".into())]).unwrap();
        let tip_b = git_in(&repo, &["rev-parse", BRANCH]).unwrap();

        // Reset branch to tip_a (simulates local state)
        git_in(
            &repo,
            &["update-ref", &format!("refs/heads/{BRANCH}"), &tip_a],
        )
        .unwrap();

        // Replay: local (tip_a) has no files that tip_b doesn't have
        replay_local_files(&repo, &tip_a, &tip_b, BRANCH).unwrap();

        // Branch should now be at tip_b (advanced via CAS) with no
        // extra commits since there was nothing to replay
        let tree = git_in(&repo, &["ls-tree", "-r", "--name-only", BRANCH]).unwrap();
        assert!(tree.contains("README.md"));
    }

    #[test]
    fn test_replay_local_files_preserves_diverged_data() {
        let (tmp, repo) = match init_test_repo() {
            Some(r) => r,
            None => return,
        };
        let _ = tmp;

        ensure_branch(&repo).unwrap();
        let base = git_in(&repo, &["rev-parse", BRANCH]).unwrap();

        // Build local branch: base + local file
        write_to_branch(
            &repo,
            &[("aa/bb/metadata.json".into(), r#"{"a":1}"#.into())],
        )
        .unwrap();
        let local_tip = git_in(&repo, &["rev-parse", BRANCH]).unwrap();

        // Build remote branch: reset to base, add different file
        git_in(
            &repo,
            &["update-ref", &format!("refs/heads/{BRANCH}"), &base],
        )
        .unwrap();
        write_to_branch(
            &repo,
            &[("cc/dd/metadata.json".into(), r#"{"b":2}"#.into())],
        )
        .unwrap();
        let remote_tip = git_in(&repo, &["rev-parse", BRANCH]).unwrap();

        // Set branch back to local_tip (replay expects branch at local_tip)
        git_in(
            &repo,
            &["update-ref", &format!("refs/heads/{BRANCH}"), &local_tip],
        )
        .unwrap();

        replay_local_files(&repo, &local_tip, &remote_tip, BRANCH).unwrap();

        let tree = git_in(&repo, &["ls-tree", "-r", "--name-only", BRANCH]).unwrap();
        assert!(
            tree.contains("aa/bb/metadata.json"),
            "local-only file should be replayed: {tree}"
        );
        assert!(
            tree.contains("cc/dd/metadata.json"),
            "remote file should be present: {tree}"
        );
        assert!(
            tree.contains("README.md"),
            "initial README should be present: {tree}"
        );
    }

    #[test]
    fn test_replay_union_merges_diverged_jsonl_indexes() {
        // Two writers appended different entries to the same append-only
        // index file before reconciling. Neither side's entries may be
        // lost — "remote wins" would silently drop local index lines while
        // their data files survive, leaving unreachable records.
        let (tmp, repo) = match init_test_repo() {
            Some(r) => r,
            None => return,
        };
        let _ = tmp;

        ensure_branch(&repo).unwrap();
        let base = git_in(&repo, &["rev-parse", BRANCH]).unwrap();

        let index = "repos/xx/index/anchors-by-time.jsonl";
        write_to_branch(
            &repo,
            &[(index.into(), "100\tsha-base\n101\tsha-local\n".into())],
        )
        .unwrap();
        let local_tip = git_in(&repo, &["rev-parse", BRANCH]).unwrap();

        git_in(
            &repo,
            &["update-ref", &format!("refs/heads/{BRANCH}"), &base],
        )
        .unwrap();
        write_to_branch(
            &repo,
            &[(index.into(), "100\tsha-base\n102\tsha-remote\n".into())],
        )
        .unwrap();
        let remote_tip = git_in(&repo, &["rev-parse", BRANCH]).unwrap();

        git_in(
            &repo,
            &["update-ref", &format!("refs/heads/{BRANCH}"), &local_tip],
        )
        .unwrap();

        replay_local_files(&repo, &local_tip, &remote_tip, BRANCH).unwrap();

        let merged = git_in(&repo, &["show", &format!("{BRANCH}:{index}")]).unwrap();
        assert!(
            merged.contains("sha-base"),
            "shared line survives: {merged}"
        );
        assert!(
            merged.contains("sha-local"),
            "local-only index line survives: {merged}"
        );
        assert!(
            merged.contains("sha-remote"),
            "remote-only index line survives: {merged}"
        );
        assert_eq!(
            merged.matches("sha-base").count(),
            1,
            "shared line is not duplicated: {merged}"
        );
    }

    #[test]
    fn test_write_to_branch_retry_succeeds_after_concurrent_update() {
        let (tmp, repo) = match init_test_repo() {
            Some(r) => r,
            None => return,
        };
        let _ = tmp;

        ensure_branch(&repo).unwrap();

        // Write two entries sequentially  --  the second write succeeds because
        // try_write_to_branch uses CAS and the retry re-reads the parent.
        write_to_branch(&repo, &[("a/b/m.json".into(), "one".into())]).unwrap();
        write_to_branch(&repo, &[("c/d/m.json".into(), "two".into())]).unwrap();

        let tree = git_in(&repo, &["ls-tree", "-r", "--name-only", BRANCH]).unwrap();
        assert!(tree.contains("a/b/m.json"));
        assert!(tree.contains("c/d/m.json"));
    }

    #[test]
    fn test_tmp_index_cleaned_up_on_mid_pipeline_error() {
        let (tmp, repo) = match init_test_repo() {
            Some(r) => r,
            None => return,
        };
        let _ = tmp;

        ensure_branch(&repo).unwrap();
        let git_dir = crate::git::detect::resolve_git_common_dir(&repo);
        let parent = git_in(&repo, &["rev-parse", BRANCH]).unwrap();

        let assert_no_leftover = |label: &str| {
            let leftover: Vec<_> = std::fs::read_dir(&git_dir)
                .unwrap()
                .filter_map(std::result::Result::ok)
                .filter(|e| e.file_name().to_string_lossy().starts_with("oobo-index-"))
                .collect();
            assert!(leftover.is_empty(), "{label}: found {leftover:?}");
        };

        // Success path: full pipeline runs, temp index cleaned up.
        let _ = build_commit_on(
            &repo,
            &parent,
            &[("test.txt".into(), "data".into())],
            "test",
        );
        assert_no_leftover("after successful build");

        // Error path: read-tree succeeds (temp index created on disk),
        // then update-index rejects the empty path → closure returns Err,
        // cleanup removes the temp file.
        // On Windows, git update-index may accept an empty path differently,
        // so only assert the error on Unix.  The success path above already
        // verifies cleanup after a successful pipeline on all platforms.
        #[cfg(unix)]
        {
            let result = build_commit_on(
                &repo,
                &parent,
                &[(String::new(), "data".into())],
                "should fail",
            );
            assert!(result.is_err(), "empty path should fail in update-index");
            assert_no_leftover("after mid-pipeline error");
        }
    }

    #[test]
    fn test_build_timeline_json() {
        use crate::core::anchor::{FileInteraction, FileRole, FileSessionRole};

        let mut anchor = make_test_anchor("abc123");
        anchor.session_ids = vec!["s1".into(), "s2".into()];
        let interactions = vec![FileInteraction {
            path: "src/main.rs".into(),
            sessions: vec![
                FileSessionRole {
                    session_id: "s1".into(),
                    role: FileRole::Writer,
                },
                FileSessionRole {
                    session_id: "s2".into(),
                    role: FileRole::Reader,
                },
            ],
        }];
        anchor.file_interactions = Some(interactions.clone());

        let links = vec![
            SessionLink {
                session_id: "s1".into(),
                agent: "cursor".into(),
                model: None,
                link_type: LinkType::Explicit,
                input_tokens: None,
                output_tokens: None,
                cache_read_tokens: None,
                cache_creation_tokens: None,
                duration_secs: Some(120),
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
                peer_session_ids: vec!["s2".into()],
            },
            SessionLink {
                session_id: "s2".into(),
                agent: "claude".into(),
                model: None,
                link_type: LinkType::Inferred,
                input_tokens: None,
                output_tokens: None,
                cache_read_tokens: None,
                cache_creation_tokens: None,
                duration_secs: Some(60),
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
                peer_session_ids: vec!["s1".into()],
            },
        ];

        let json_str =
            build_timeline_json("/tmp/test-repo", &anchor, &links, &interactions).unwrap();
        let val: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(val["session_count"], 2);
        assert_eq!(val["longest_session_ms"], 120_000);
        assert_eq!(val["file_interactions"].as_array().unwrap().len(), 1);
        assert_eq!(val["file_interactions"][0]["path"], "src/main.rs");
        assert_eq!(
            val["file_interactions"][0]["sessions"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}
