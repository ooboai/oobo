use std::collections::HashSet;
use std::fs;

use crate::error::CliError;

use super::{branch_exists, build_commit_on, git_in, BRANCH, NULL_OID};

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
#[tracing::instrument(skip_all)]
pub fn push(project_root: &str) -> Result<(), CliError> {
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
            Err(ref e) if {
                let msg = e.to_string();
                msg.contains("non-fast-forward") || msg.contains("rejected")
            } => {
                last_err = e.to_string();
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
    Err(CliError::Git(format!(
        "push failed after {MAX_PUSH_ATTEMPTS} attempts: {last_err}"
    )))
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
    let nanos = u64::from(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos());
    let pid = u64::from(std::process::id());
    nanos.wrapping_mul(pid.wrapping_add(7)) % max
}

/// Fetch the orphan branch from the configured anchor remote and reconcile
/// diverged branches.
/// Working tree and HEAD are never touched.
pub fn fetch_and_reconcile(project_root: &str) -> Result<(), CliError> {
    let remote = anchor_remote(project_root);
    validate_anchor_remote(project_root, &remote)?;
    reconcile_with_remote(project_root, &remote).map_err(|e| CliError::Git(format!("fetch/reconcile failed: {e}")))
}

/// Fetch into a PID-namespaced temp ref to avoid FETCH_HEAD races and
/// force-fetch data loss.
fn reconcile_with_remote(project_root: &str, remote: &str) -> Result<(), CliError> {
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

fn reconcile_local_with(project_root: &str, remote_tip: &str) -> Result<(), CliError> {
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
pub(super) fn replay_local_files(project_root: &str, local_tip: &str, remote_tip: &str) -> Result<(), CliError> {
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
                    tracing::warn!(path, %e, "could not read from local anchors");
                    skipped += 1;
                }
            }
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
            &format!("refs/heads/{BRANCH}"),
            &target,
            local_tip,
        ],
    )?;

    Ok(())
}
