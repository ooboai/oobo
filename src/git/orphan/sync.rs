use std::collections::HashMap;
use std::fs;

use crate::error::CliError;

use super::{branch_exists_named, build_commit_on, git_in, git_in_timeout, BRANCH, NULL_OID};

const NETWORK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Resolve the anchor remote once per call site. Uses the full precedence
/// chain: project config > global config > "origin".
fn anchor_remote(project_root: &str) -> String {
    let global = crate::config::Config::load_or_default();
    crate::commands::sync::resolve(&global, Some(project_root)).anchor_remote
}

pub fn remote_branch_exists(project_root: &str) -> bool {
    remote_branch_exists_named(project_root, BRANCH)
}

pub fn remote_branch_exists_named(project_root: &str, branch: &str) -> bool {
    let remote = anchor_remote(project_root);
    if validate_anchor_remote(project_root, &remote).is_err() {
        return false;
    }
    git_in_timeout(
        project_root,
        &["ls-remote", "--heads", &remote, branch],
        NETWORK_TIMEOUT,
    )
    .map(|out| !out.trim().is_empty())
    .unwrap_or(false)
}

/// 5 retries: remote push contention is common in multi-user/agent
/// workflows and each attempt requires a network round-trip.
const MAX_PUSH_ATTEMPTS: u32 = 5;

/// Push the orphan branch to the configured anchor remote with retry on
/// contention. Defaults to `origin`; `.oobo/config [anchors].remote` can point
/// at another Git remote name or Git URL.
#[tracing::instrument(skip_all)]
pub fn push(project_root: &str) -> Result<(), CliError> {
    push_branch(project_root, BRANCH)
}

pub fn push_branch(project_root: &str, branch: &str) -> Result<(), CliError> {
    let remote = anchor_remote(project_root);
    validate_anchor_remote(project_root, &remote)?;
    retry_pending_pushes_branch(project_root, branch);

    let mut last_err = String::new();
    for attempt in 0..MAX_PUSH_ATTEMPTS {
        match git_in_timeout(
            project_root,
            &["push", "--no-verify", &remote, branch],
            NETWORK_TIMEOUT,
        ) {
            Ok(_) => {
                clear_pending_push(project_root, branch);
                return Ok(());
            }
            Err(ref e)
                if {
                    let msg = e.to_string();
                    msg.contains("non-fast-forward") || msg.contains("rejected")
                } =>
            {
                last_err = e.to_string();
                if attempt < MAX_PUSH_ATTEMPTS - 1 {
                    if let Err(re) = reconcile_with_remote(project_root, &remote, branch) {
                        last_err = format!("{last_err}; reconcile failed: {re}");
                    }
                    jitter_sleep(attempt);
                }
            }
            Err(e) => return Err(e),
        }
    }
    mark_pending_push(project_root, branch);
    Err(CliError::Git(format!(
        "push failed after {MAX_PUSH_ATTEMPTS} attempts: {last_err}"
    )))
}

fn retry_pending_pushes_branch(project_root: &str, branch: &str) {
    let remote = anchor_remote(project_root);
    if validate_anchor_remote(project_root, &remote).is_err() {
        return;
    }
    let path = pending_push_path(project_root, branch);
    if !path.exists() {
        return;
    }
    let _ = reconcile_with_remote(project_root, &remote, branch);
    if git_in_timeout(
        project_root,
        &["push", "--no-verify", &remote, branch],
        NETWORK_TIMEOUT,
    )
    .is_ok()
    {
        let _ = fs::remove_file(&path);
    }
}

fn pending_push_path(project_root: &str, branch: &str) -> std::path::PathBuf {
    let common = crate::git::detect::resolve_git_common_dir(project_root);
    if branch == BRANCH {
        // Keep the historical marker name for the v1 branch.
        common.join("oobo-push-pending")
    } else {
        common.join(format!("oobo-push-pending-{}", branch.replace('/', "-")))
    }
}

fn mark_pending_push(project_root: &str, branch: &str) {
    let path = pending_push_path(project_root, branch);
    let _ = fs::write(&path, chrono::Utc::now().to_rfc3339());
}

fn clear_pending_push(project_root: &str, branch: &str) {
    let path = pending_push_path(project_root, branch);
    let _ = fs::remove_file(&path);
}

pub(super) fn jitter_sleep(attempt: u32) {
    let base_ms = 100u64 * 2u64.pow(attempt);
    let jitter = rand_jitter_ms(base_ms / 2);
    std::thread::sleep(std::time::Duration::from_millis(base_ms + jitter));
}

/// Mixes in PID so concurrent processes get decorrelated jitter.
fn rand_jitter_ms(max: u64) -> u64 {
    if max == 0 {
        return 0;
    }
    let nanos = u64::from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos(),
    );
    let pid = u64::from(std::process::id());
    nanos.wrapping_mul(pid.wrapping_add(7)) % max
}

/// Fetch the orphan branch from the configured anchor remote and reconcile
/// diverged branches.
/// Working tree and HEAD are never touched.
pub fn fetch_and_reconcile(project_root: &str) -> Result<(), CliError> {
    fetch_and_reconcile_branch(project_root, BRANCH)
}

pub fn fetch_and_reconcile_branch(project_root: &str, branch: &str) -> Result<(), CliError> {
    let remote = anchor_remote(project_root);
    validate_anchor_remote(project_root, &remote)?;
    reconcile_with_remote(project_root, &remote, branch)
        .map_err(|e| CliError::Git(format!("fetch/reconcile failed: {e}")))
}

/// Fetch into a PID-namespaced temp ref to avoid FETCH_HEAD races and
/// force-fetch data loss.
fn reconcile_with_remote(project_root: &str, remote: &str, branch: &str) -> Result<(), CliError> {
    let fetch_ref = format!("refs/oobo/fetch-tmp/{}", std::process::id());
    let refspec = format!("+{branch}:{fetch_ref}");

    let fetch_result = git_in_timeout(project_root, &["fetch", remote, &refspec], NETWORK_TIMEOUT);

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

    let result = reconcile_local_with(project_root, &remote_tip, branch);
    cleanup(project_root);
    result
}

pub(super) fn validate_anchor_remote(project_root: &str, remote: &str) -> Result<(), CliError> {
    if !looks_like_remote_name(remote) {
        return Ok(());
    }
    git_in(project_root, &["remote", "get-url", remote])
        .map(|_| ())
        .map_err(|_| {
            CliError::Config(format!(
                "oobo remote '{remote}' is not configured. Run `git remote add {remote} <url>` \
                 or set `[anchors].remote` to a full Git URL in .oobo/config."
            ))
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

fn reconcile_local_with(
    project_root: &str,
    remote_tip: &str,
    branch: &str,
) -> Result<(), CliError> {
    if !branch_exists_named(project_root, branch) {
        git_in(
            project_root,
            &[
                "update-ref",
                &format!("refs/heads/{branch}"),
                remote_tip,
                NULL_OID,
            ],
        )?;
        return Ok(());
    }

    let local_tip = git_in(project_root, &["rev-parse", branch])?;

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
                &format!("refs/heads/{branch}"),
                remote_tip,
                &local_tip,
            ],
        )?;
        return Ok(());
    }

    replay_local_files(project_root, &local_tip, remote_tip, branch)
}

/// List `(blob_hash, path)` for every file reachable from `tip`.
fn tree_entries(project_root: &str, tip: &str) -> Result<HashMap<String, String>, CliError> {
    // `git ls-tree -r <tip>` lines: "<mode> blob <hash>\t<path>"
    let raw = git_in(project_root, &["ls-tree", "-r", tip])?;
    let mut out = HashMap::new();
    for line in raw.lines() {
        let Some((meta, path)) = line.split_once('\t') else {
            continue;
        };
        let hash = meta.split_whitespace().nth(2).unwrap_or("");
        if !hash.is_empty() {
            out.insert(path.to_string(), hash.to_string());
        }
    }
    Ok(out)
}

/// Union-merge two diverged orphan tips. Builds a merged commit before
/// moving the branch ref  --  if any step fails, the branch is untouched.
///
/// Merge rules:
/// - Files only in local → replayed on top of remote (the v1 behavior).
/// - Files in both with identical content → nothing to do.
/// - Files in both with **different** content → remote wins, EXCEPT:
///   - `session.json` (the v2 mutable session record), merged per-field:
///     sets union, counters max, scalars last-writer-wins;
///   - `*.jsonl` (append-only index files), merged by line union so
///     entries appended on either side both survive. Readers dedup by id
///     at read time, so order across the seam doesn't matter.
pub(super) fn replay_local_files(
    project_root: &str,
    local_tip: &str,
    remote_tip: &str,
    branch: &str,
) -> Result<(), CliError> {
    let local_tree = tree_entries(project_root, local_tip)?;
    let remote_tree = tree_entries(project_root, remote_tip)?;

    let mut entries: Vec<(String, String)> = Vec::new();
    let mut skipped = 0u32;
    for (path, local_hash) in &local_tree {
        match remote_tree.get(path) {
            None => match git_in(project_root, &["show", &format!("{local_tip}:{path}")]) {
                Ok(content) => entries.push((path.clone(), content)),
                Err(e) => {
                    tracing::warn!(path, %e, "could not read from local anchors");
                    skipped += 1;
                }
            },
            Some(remote_hash) if remote_hash != local_hash && path.ends_with("session.json") => {
                let local = git_in(project_root, &["show", &format!("{local_tip}:{path}")]);
                let remote = git_in(project_root, &["show", &format!("{remote_tip}:{path}")]);
                if let (Ok(local), Ok(remote)) = (local, remote) {
                    if let Some(merged) = super::v2::merge_session_json(&local, &remote) {
                        entries.push((path.clone(), merged));
                    }
                }
            }
            Some(remote_hash)
                if remote_hash != local_hash
                    && std::path::Path::new(path)
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl")) =>
            {
                let local = git_in(project_root, &["show", &format!("{local_tip}:{path}")]);
                let remote = git_in(project_root, &["show", &format!("{remote_tip}:{path}")]);
                if let (Ok(local), Ok(remote)) = (local, remote) {
                    entries.push((path.clone(), union_merge_lines(&remote, &local)));
                }
            }
            Some(_) => {}
        }
    }

    if skipped > 0 {
        tracing::warn!(skipped, "local anchor file(s) could not be replayed");
    }

    let target = if entries.is_empty() {
        remote_tip.to_string()
    } else {
        build_commit_on(
            project_root,
            remote_tip,
            &entries,
            "oobo: replay local anchors after reconcile",
        )?
    };

    git_in(
        project_root,
        &[
            "update-ref",
            &format!("refs/heads/{branch}"),
            &target,
            local_tip,
        ],
    )?;

    Ok(())
}

/// Line union of two append-only index files: every distinct line from
/// either side survives, remote order first, local-only lines appended.
fn union_merge_lines(remote: &str, local: &str) -> String {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut out = String::new();
    for line in remote.lines().chain(local.lines()) {
        if line.is_empty() || !seen.insert(line) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}
