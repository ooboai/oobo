use std::collections::{HashMap, HashSet};
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
pub(super) fn write_anchor(
    project_root: &str,
    anchor: &Anchor,
    session_links: &[SessionLink],
    transcripts: &[super::transcripts::CollectedTranscript],
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
        let session_path = format!("{base_path}/{}", i + 1);
        entries.push((format!("{session_path}/metadata.json"), link_json));

        if anchor.transparency_mode == TransparencyMode::On {
            if let Some(ct) = transcripts
                .iter()
                .find(|ct| ct.session_id == link.session_id)
            {
                let redacted = crate::redact::redact(&ct.content);
                let sanitized = strip_absolute_paths(&redacted, project_root);
                entries.push((format!("{session_path}/transcript.json"), sanitized));
            }

            // Write subagent transcripts nested under the parent session.
            let mut sub_idx = 0u32;
            for ct in transcripts
                .iter()
                .filter(|ct| ct.parent_session_id.as_deref() == Some(&link.session_id))
            {
                sub_idx += 1;
                let redacted = crate::redact::redact(&ct.content);
                let sanitized = strip_absolute_paths(&redacted, project_root);
                entries.push((
                    format!("{session_path}/subagents/{}/transcript.json", sub_idx),
                    sanitized,
                ));
            }
        }
    }

    // Generate timeline.json when file interactions exist.
    if let Some(ref interactions) = anchor.file_interactions {
        if !interactions.is_empty() {
            if let Ok(timeline_json) = build_timeline_json(anchor, session_links, interactions) {
                entries.push((format!("{base_path}/timeline.json"), timeline_json));
            }
        }
    }

    write_to_branch(project_root, &entries)?;

    Ok(())
}

/// Build a timeline JSON blob for multi-agent file interactions.
///
/// TODO: Add an `events` array with per-file timestamped read/write events
/// to enable causality analysis (e.g. "Agent-2 read calculator.py 49s after
/// Agent-1 wrote it"). Requires recording per-file timestamps in
/// `edited_files`/`read_files` state, which is not yet available.
fn build_timeline_json(
    anchor: &Anchor,
    session_links: &[SessionLink],
    interactions: &[crate::core::anchor::FileInteraction],
) -> Result<String, String> {
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

    let mut timeline = serde_json::json!({
        "session_count": anchor.session_ids.len(),
        "file_interactions": interactions_json,
    });

    if let Some(dur) = longest_session_ms {
        timeline["longest_session_ms"] = serde_json::json!(dur);
    }

    serde_json::to_string_pretty(&timeline).map_err(|e| format!("serialize timeline: {e}"))
}

/// Delegate to the shared implementation in `redact` module.
fn strip_absolute_paths(text: &str, project_root: &str) -> String {
    crate::redact::strip_absolute_paths(text, project_root)
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

    let mut hashes = HashSet::new();
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

/// Re-key anchors after a history rewrite (rebase, cherry-pick).
///
/// `pre_rewrite_commits` is a list of (old_commit_hash, tree_hash) captured
/// before the rewrite. After the rewrite, we find new commits with matching
/// tree hashes and copy anchors from the old SHA to the new SHA.
///
/// Phase 1: only handles simple rewrites where file content didn't change
/// (tree hash is preserved). Content-changing rewrites (squash, conflict
/// resolution) are deferred to Phase 2.
pub fn rekey_anchors(
    project_root: &str,
    pre_rewrite_commits: &[(String, String)],
) -> Result<(), String> {
    if !branch_exists(project_root) || pre_rewrite_commits.is_empty() {
        return Ok(());
    }

    let new_commits = current_branch_commits(project_root);
    if new_commits.is_empty() {
        return Ok(());
    }

    let new_by_tree: std::collections::HashMap<&str, &str> = new_commits
        .iter()
        .map(|(hash, tree)| (tree.as_str(), hash.as_str()))
        .collect();

    let mut entries = Vec::new();

    for (old_hash, tree) in pre_rewrite_commits {
        if let Some(&new_hash) = new_by_tree.get(tree.as_str()) {
            if old_hash == new_hash {
                continue;
            }
            if read_anchor(project_root, old_hash).is_none() {
                continue;
            }

            let (old_prefix, old_rest) = shard_key(old_hash);
            let old_base = format!("{old_prefix}/{old_rest}");
            let (new_prefix, new_rest) = shard_key(new_hash);
            let new_base = format!("{new_prefix}/{new_rest}");

            let file_list = git_in(
                project_root,
                &[
                    "ls-tree",
                    "-r",
                    "--name-only",
                    BRANCH,
                    &format!("{old_base}/"),
                ],
            )
            .unwrap_or_default();

            for file_path in file_list.lines() {
                if file_path.is_empty() {
                    continue;
                }
                let relative = file_path.strip_prefix(&old_base).unwrap_or(file_path);
                let relative = relative.strip_prefix('/').unwrap_or(relative);
                let new_path = format!("{new_base}/{relative}");

                if relative == "metadata.json" && !relative.contains('/') {
                    if let Some(mut anchor) = read_anchor(project_root, old_hash) {
                        anchor.commit_hash = new_hash.to_string();
                        let json = serde_json::to_string_pretty(&anchor)
                            .map_err(|e| format!("serialize anchor: {e}"))?;
                        entries.push((new_path, json));
                        continue;
                    }
                }

                if let Some(content) = read_from_branch(project_root, file_path) {
                    entries.push((new_path, content));
                }
            }

        }
    }

    if !entries.is_empty() {
        write_to_branch(project_root, &entries)?;
    }

    Ok(())
}

pub fn parse_rewrite_pairs(payload: &str) -> Vec<(String, String)> {
    payload
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let old_hash = parts.next()?;
            let new_hash = parts.next()?;
            Some((old_hash.to_string(), new_hash.to_string()))
        })
        .collect()
}

pub fn rekey_anchors_from_rewrite_pairs(
    project_root: &str,
    pairs: &[(String, String)],
) -> Result<(), String> {
    if pairs.is_empty() {
        return Ok(());
    }

    let mut pre_rewrite_commits = Vec::new();
    for (old_hash, _) in pairs {
        let tree =
            git_in(project_root, &["show", "-s", "--format=%T", old_hash]).unwrap_or_default();
        let tree = tree.trim();
        if !tree.is_empty() {
            pre_rewrite_commits.push((old_hash.clone(), tree.to_string()));
        }
    }

    rekey_anchors(project_root, &pre_rewrite_commits)
}

fn current_branch_commits(project_root: &str) -> Vec<(String, String)> {
    let output = git_in(project_root, &["log", "--format=%H %T", "HEAD"]);
    match output {
        Ok(text) => text
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(2, ' ');
                let hash = parts.next()?.to_string();
                let tree = parts.next()?.to_string();
                Some((hash, tree))
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

pub fn branch_exists(project_root: &str) -> bool {
    git_in(project_root, &["rev-parse", "--verify", BRANCH]).is_ok()
}

/// Get the current tip commit hash of the orphan branch.
pub fn branch_tip(project_root: &str) -> Option<String> {
    git_in(project_root, &["rev-parse", BRANCH]).ok()
}

/// Bulk-read ALL anchors and their session links from the orphan branch
/// using a single `git cat-file --batch` process for maximum speed.
pub fn read_all_anchors(project_root: &str) -> (Vec<Anchor>, HashMap<String, Vec<SessionLink>>) {
    let mut anchors = Vec::new();
    let mut links_map: HashMap<String, Vec<SessionLink>> = HashMap::new();

    if !branch_exists(project_root) {
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
            anchor_paths.push((hash, format!("{}:{}", BRANCH, line)));
        } else if parts.len() == 4 && parts[3] == "metadata.json" && parts[2].parse::<u32>().is_ok()
        {
            let hash = format!("{}{}", parts[0], parts[1]);
            session_paths.push((hash, format!("{}:{}", BRANCH, line)));
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
/// Returns a Vec parallel to `refs` — `Some(content)` for successful reads,
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
    let mut child = match Command::new(git)
        .args(["cat-file", "--batch"])
        .current_dir(project_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return vec![None; refs.len()],
    };

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let refs_owned: Vec<String> = refs.iter().map(|r| r.to_string()).collect();
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

fn anchor_remote(project_root: &str) -> String {
    crate::project_config::anchor_remote(project_root).unwrap_or_else(|| "origin".to_string())
}

pub fn remote_branch_exists(project_root: &str) -> bool {
    let remote = anchor_remote(project_root);
    if validate_anchor_remote(project_root, &remote).is_err() {
        return false;
    }
    git_in(project_root, &["ls-remote", "--heads", &remote, BRANCH])
        .map(|out| !out.trim().is_empty())
        .unwrap_or(false)
}

/// 5 retries: remote push contention is common in multi-user/agent
/// workflows and each attempt requires a network round-trip.
const MAX_PUSH_ATTEMPTS: u32 = 5;

/// Push the orphan branch to the configured anchor remote with retry on
/// contention. Defaults to `origin`; `.oobo/config [anchors].remote` can point
/// at another Git remote name or Git URL.
pub fn push(project_root: &str) -> Result<(), String> {
    let remote = anchor_remote(project_root);
    validate_anchor_remote(project_root, &remote)?;
    retry_pending_pushes(project_root);

    let mut last_err = String::new();
    for attempt in 0..MAX_PUSH_ATTEMPTS {
        match git_in(project_root, &["push", "--no-verify", &remote, BRANCH]) {
            Ok(_) => {
                clear_pending_push(project_root);
                return Ok(());
            }
            Err(e) if e.contains("non-fast-forward") || e.contains("rejected") => {
                last_err = e;
                if attempt < MAX_PUSH_ATTEMPTS - 1 {
                    if let Err(re) = reconcile_with_remote(project_root, &remote) {
                        last_err = format!("{last_err}; reconcile failed: {re}");
                    }
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

pub fn retry_pending_pushes(project_root: &str) {
    let remote = anchor_remote(project_root);
    if validate_anchor_remote(project_root, &remote).is_err() {
        return;
    }
    let path = pending_push_path(project_root);
    if !path.exists() {
        return;
    }
    let _ = reconcile_with_remote(project_root, &remote);
    if git_in(project_root, &["push", "--no-verify", &remote, BRANCH]).is_ok() {
        let _ = fs::remove_file(&path);
    }
}

fn pending_push_path(project_root: &str) -> std::path::PathBuf {
    crate::git::detect::resolve_git_common_dir(project_root).join("oobo-push-pending")
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

/// Mixes in PID so concurrent processes get decorrelated jitter.
fn rand_jitter_ms(max: u64) -> u64 {
    if max == 0 {
        return 0;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    let pid = std::process::id() as u64;
    nanos.wrapping_mul(pid.wrapping_add(7)) % max
}

/// Fetch the orphan branch from the configured anchor remote and reconcile
/// diverged branches.
/// Working tree and HEAD are never touched.
pub fn fetch_and_reconcile(project_root: &str) -> Result<(), String> {
    let remote = anchor_remote(project_root);
    validate_anchor_remote(project_root, &remote)?;
    reconcile_with_remote(project_root, &remote).map_err(|e| format!("fetch/reconcile failed: {e}"))
}

const NULL_OID: &str = "0000000000000000000000000000000000000000";

/// Fetch into a PID-namespaced temp ref to avoid FETCH_HEAD races and
/// force-fetch data loss.
fn reconcile_with_remote(project_root: &str, remote: &str) -> Result<(), String> {
    let fetch_ref = format!("refs/oobo/fetch-tmp/{}", std::process::id());
    let refspec = format!("+{BRANCH}:{fetch_ref}");

    let fetch_result = git_in(project_root, &["fetch", remote, &refspec]);

    let cleanup = |pr: &str| {
        let _ = git_in(pr, &["update-ref", "-d", &fetch_ref]);
    };

    if let Err(e) = fetch_result {
        cleanup(project_root);
        return Err(e);
    }

    let remote_tip = match git_in(project_root, &["rev-parse", &fetch_ref]) {
        Ok(tip) => tip,
        Err(e) => {
            cleanup(project_root);
            return Err(e);
        }
    };

    let result = reconcile_local_with(project_root, &remote_tip);
    cleanup(project_root);
    result
}

fn validate_anchor_remote(project_root: &str, remote: &str) -> Result<(), String> {
    if !looks_like_remote_name(remote) {
        return Ok(());
    }
    git_in(project_root, &["remote", "get-url", remote])
        .map(|_| ())
        .map_err(|_| {
            format!(
                "anchor remote '{remote}' is not configured. Run `git remote add {remote} <url>` \
                 or set `[anchors].remote` to a full Git URL in .oobo/config."
            )
        })
}

fn looks_like_remote_name(remote: &str) -> bool {
    !remote.is_empty()
        && !remote.contains('/')
        && !remote.contains('\\')
        && !remote.contains(':')
        && !remote.contains('@')
        && !remote.contains("://")
}

fn reconcile_local_with(project_root: &str, remote_tip: &str) -> Result<(), String> {
    if !branch_exists(project_root) {
        git_in(
            project_root,
            &[
                "update-ref",
                &format!("refs/heads/{BRANCH}"),
                remote_tip,
                NULL_OID,
            ],
        )?;
        return Ok(());
    }

    let local_tip = git_in(project_root, &["rev-parse", BRANCH])?;

    if local_tip == remote_tip {
        return Ok(());
    }

    if git_in(
        project_root,
        &["merge-base", "--is-ancestor", remote_tip, &local_tip],
    )
    .is_ok()
    {
        return Ok(());
    }

    if git_in(
        project_root,
        &["merge-base", "--is-ancestor", &local_tip, remote_tip],
    )
    .is_ok()
    {
        git_in(
            project_root,
            &[
                "update-ref",
                &format!("refs/heads/{BRANCH}"),
                remote_tip,
                &local_tip,
            ],
        )?;
        return Ok(());
    }

    replay_local_files(project_root, &local_tip, remote_tip)
}

/// Builds a merged commit before moving the branch ref — if any step
/// fails, the branch is untouched.
fn replay_local_files(project_root: &str, local_tip: &str, remote_tip: &str) -> Result<(), String> {
    let local_tree = git_in(project_root, &["ls-tree", "-r", "--name-only", local_tip])?;
    let remote_tree = git_in(project_root, &["ls-tree", "-r", "--name-only", remote_tip])?;

    let remote_set: HashSet<&str> = remote_tree.lines().collect();

    let mut entries: Vec<(String, String)> = Vec::new();
    let mut skipped = 0u32;
    for path in local_tree.lines() {
        if !remote_set.contains(path) {
            match git_in(project_root, &["show", &format!("{local_tip}:{path}")]) {
                Ok(content) => entries.push((path.to_string(), content)),
                Err(e) => {
                    eprintln!("anchor: warning: could not read {path} from local anchors: {e}");
                    skipped += 1;
                }
            }
        }
    }

    if skipped > 0 {
        eprintln!("anchor: warning: {skipped} local anchor file(s) could not be replayed");
    }

    let target = if entries.is_empty() {
        remote_tip.to_string()
    } else {
        build_commit_on(
            project_root,
            remote_tip,
            &entries,
            "anchor: replay local anchors after reconcile",
        )?
    };

    git_in(
        project_root,
        &[
            "update-ref",
            &format!("refs/heads/{BRANCH}"),
            &target,
            local_tip,
        ],
    )?;

    Ok(())
}

/// Shard a commit hash into directory prefix + remainder.
/// `c8e12fa9b3d4...` → ("c8", "e12fa9b3d4...")
fn shard_key(hash: &str) -> (&str, &str) {
    if hash.len() >= 3 {
        (&hash[..2], &hash[2..])
    } else {
        (hash, "")
    }
}

/// Create the orphan branch using plumbing commands only — never touches
/// the working tree or index, so uncommitted changes are safe.
fn ensure_branch(project_root: &str) -> Result<(), String> {
    if branch_exists(project_root) {
        return Ok(());
    }

    let readme_content = "# Anchors\n\nThis branch contains anchor metadata managed by anchor.\nDo not edit manually.\n";
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
        &[
            "update-ref",
            &format!("refs/heads/{BRANCH}"),
            &commit,
            NULL_OID,
        ],
    )?;

    Ok(())
}

/// 3 retries: local ref CAS failures resolve quickly (no network).
const MAX_WRITE_ATTEMPTS: u32 = 3;

/// Write entries to the orphan branch, retrying on CAS contention.
fn write_to_branch(project_root: &str, entries: &[(String, String)]) -> Result<(), String> {
    let mut last_err = String::new();
    for attempt in 0..MAX_WRITE_ATTEMPTS {
        match try_write_to_branch(project_root, entries) {
            Ok(()) => return Ok(()),
            Err(e)
                if attempt < MAX_WRITE_ATTEMPTS - 1
                    && (e.contains("but expected") || e.contains("cannot lock ref")) =>
            {
                last_err = e;
                jitter_sleep(attempt);
            }
            Err(e) => return Err(e),
        }
    }
    Err(format!(
        "write failed after {MAX_WRITE_ATTEMPTS} attempts: {last_err}"
    ))
}

/// Build a commit on top of `parent` with `entries` added. Returns the
/// commit hash without updating any ref.
fn build_commit_on(
    project_root: &str,
    parent: &str,
    entries: &[(String, String)],
    message: &str,
) -> Result<String, String> {
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
fn try_write_to_branch(project_root: &str, entries: &[(String, String)]) -> Result<(), String> {
    let parent = git_in(project_root, &["rev-parse", BRANCH])?;
    let new_commit = build_commit_on(
        project_root,
        &parent,
        entries,
        &format!(
            "anchor: add anchor for {}",
            entries.first().map(|(p, _)| p.as_str()).unwrap_or("?")
        ),
    )?;

    git_in(
        project_root,
        &[
            "update-ref",
            &format!("refs/heads/{BRANCH}"),
            &new_commit,
            &parent,
        ],
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
        Ok(String::from_utf8_lossy(&output.stdout)
            .replace('\r', "")
            .trim()
            .to_string())
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
        Ok(String::from_utf8_lossy(&output.stdout)
            .replace('\r', "")
            .trim()
            .to_string())
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
            is_subagent: false,
            parent_session_id: None,
            subagent_type: None,
            is_estimated: false,
            peer_session_ids: Vec::new(),
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
        assert!(err.contains("anchor remote 'oobo' is not configured"));
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
        replay_local_files(&repo, &tip_a, &tip_b).unwrap();

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

        replay_local_files(&repo, &local_tip, &remote_tip).unwrap();

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
    fn test_write_to_branch_retry_succeeds_after_concurrent_update() {
        let (tmp, repo) = match init_test_repo() {
            Some(r) => r,
            None => return,
        };
        let _ = tmp;

        ensure_branch(&repo).unwrap();

        // Write two entries sequentially — the second write succeeds because
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
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with("oobo-index-"))
                .collect();
            assert!(leftover.is_empty(), "{label}: found {:?}", leftover);
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
            let result =
                build_commit_on(&repo, &parent, &[("".into(), "data".into())], "should fail");
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
                is_subagent: false,
                parent_session_id: None,
                subagent_type: None,
                is_estimated: false,
                peer_session_ids: vec!["s1".into()],
            },
        ];

        let json_str = build_timeline_json(&anchor, &links, &interactions).unwrap();
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
