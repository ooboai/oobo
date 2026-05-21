use crate::error::CliError;

use super::{git_in, read_anchor, read_from_branch, shard_key, write_to_branch, BRANCH};

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
) -> Result<(), CliError> {
    if !super::branch_exists(project_root) || pre_rewrite_commits.is_empty() {
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
) -> Result<(), CliError> {
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
