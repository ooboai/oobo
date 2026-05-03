use crate::config::Config;

use super::types::{AnchorRow, SessionLink, TimeWindow};

pub(super) fn load_anchors(
    cfg: &Config,
    project_root: &str,
    limit: usize,
    window: TimeWindow,
) -> Result<Vec<AnchorRow>, String> {
    let feed_opts = crate::feed::LoadOptions {
        limit,
        since: window.cutoff(),
        tool: None,
    };
    let rows = crate::feed::load(cfg, project_root, &feed_opts)?;
    Ok(rows.into_iter().map(AnchorRow::from).collect())
}

pub(super) fn load_sessions_for_anchor(project_root: &str, commit_hash: &str) -> Vec<SessionLink> {
    crate::git::orphan::read_session_links(project_root, commit_hash)
        .into_iter()
        .map(|l| {
            let tokens = l.input_tokens.unwrap_or(0) as i64
                + l.output_tokens.unwrap_or(0) as i64
                + l.cache_read_tokens.unwrap_or(0) as i64
                + l.cache_creation_tokens.unwrap_or(0) as i64;
            SessionLink {
                session_id: l.session_id,
                source: l.agent,
                model: l.model,
                tokens,
            }
        })
        .collect()
}

pub(super) fn touched_files_for(root: &str, sha: &str) -> Vec<String> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let Ok(output) = std::process::Command::new(git)
        .args(["show", "--name-only", "--pretty=format:", sha])
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(std::string::ToString::to_string)
        .collect()
}

pub(super) fn current_branch(root: &str) -> Option<String> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let output = std::process::Command::new(git)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

pub(super) fn worktree_dirty(root: &str) -> bool {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let Ok(output) = std::process::Command::new(git)
        .args(["status", "--porcelain"])
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return false;
    };
    output.status.success() && !String::from_utf8_lossy(&output.stdout).trim().is_empty()
}
