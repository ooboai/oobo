use crate::error::CliError;

use super::{git_in, read_anchor_v1, read_from_branch, shard_key, write_to_branch, BRANCH};

/// Re-key anchors after a history rewrite observed through the git proxy
/// (rebase, cherry-pick), where git gives us no old→new mapping.
///
/// `pre_rewrite_commits` is a list of (old_commit_hash, tree_hash) captured
/// before the rewrite. After the rewrite, we find new commits with matching
/// tree hashes and copy anchors from the old SHA to the new SHA.
///
/// Tree matching only resolves rewrites that preserved content. The
/// `post-rewrite` hook path ([`rekey_anchors_from_rewrite_pairs`]) does not
/// have this limitation because git hands it the exact pairs.
pub fn rekey_anchors(
    project_root: &str,
    pre_rewrite_commits: &[(String, String)],
) -> Result<(), CliError> {
    if pre_rewrite_commits.is_empty() {
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

    let pairs: Vec<(String, String)> = pre_rewrite_commits
        .iter()
        .filter_map(|(old_hash, tree)| {
            new_by_tree
                .get(tree.as_str())
                .map(|&new_hash| (old_hash.clone(), new_hash.to_string()))
        })
        .collect();

    let canon_root = std::fs::canonicalize(project_root).map_or_else(
        |_| project_root.to_string(),
        |p| p.to_string_lossy().to_string(),
    );
    let repo_id = crate::project::id_for_root(&canon_root);
    super::v2::rekey_anchors_from_pairs(project_root, &repo_id, &pairs)?;

    // Legacy v1 anchors (written before the v2 cut) follow the rewrite
    // too, so old history stays resolvable.
    if super::branch_exists(project_root) {
        rekey_exact_pairs(project_root, &pairs)?;
    }
    Ok(())
}

/// Copy anchor data from old SHAs to new SHAs given an exact old→new
/// mapping. Skips identity pairs and old SHAs without anchors.
fn rekey_exact_pairs(project_root: &str, pairs: &[(String, String)]) -> Result<(), CliError> {
    let mut entries = Vec::new();

    for (old_hash, new_hash) in pairs {
        if old_hash == new_hash {
            continue;
        }
        if read_anchor_v1(project_root, old_hash).is_none() {
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
                if let Some(mut anchor) = read_anchor_v1(project_root, old_hash) {
                    anchor.commit_hash.clone_from(new_hash);
                    let json = serde_json::to_string_pretty(&anchor)
                        .map_err(|e| CliError::Git(format!("serialize anchor: {e}")))?;
                    entries.push((new_path, json));
                    continue;
                }
            }

            if let Some(content) = read_from_branch(project_root, file_path) {
                entries.push((new_path, content));
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

/// Re-key anchors using the exact old→new SHA pairs git provides on the
/// `post-rewrite` hook's stdin (fires for `commit --amend` and `rebase`).
///
/// The pairs are authoritative — used directly, with no tree matching.
/// This survives content-changing rewrites (amend with staged changes,
/// rebase with conflict resolution) that tree matching cannot resolve.
pub fn rekey_anchors_from_rewrite_pairs(
    project_root: &str,
    pairs: &[(String, String)],
) -> Result<(), CliError> {
    if pairs.is_empty() || !super::branch_exists(project_root) {
        return Ok(());
    }

    rekey_exact_pairs(project_root, pairs)
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
