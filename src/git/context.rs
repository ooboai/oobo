use crate::config::Config;

use super::proxy;

#[derive(Debug, Clone, Default)]
pub(in crate::git) struct GitContext {
    pub commit_hash: String,
    pub commit_message: String,
    pub author: String,
    /// Committer timestamp (epoch seconds) from the commit itself —
    /// NOT wall-clock at enrichment time. The async worker may process
    /// a commit long after it was made; replays must be deterministic.
    pub committed_at: i64,
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
}

/// Collect commit context for a specific sha in a specific repo —
/// independent of the caller's cwd and of where HEAD has moved since
/// (the async worker may process a commit long after it was made).
pub(in crate::git) fn collect_git_context_at(
    cfg: &Config,
    project_root: &str,
    sha: &str,
) -> GitContext {
    let mut ctx = GitContext::default();
    let root = Some(project_root);

    ctx.commit_hash = proxy::run_git_capture_in(cfg, &["rev-parse", sha], root).unwrap_or_default();
    if ctx.commit_hash.is_empty() {
        return ctx;
    }
    ctx.commit_message = proxy::run_git_capture_in(cfg, &["log", "-1", "--format=%s", sha], root)
        .unwrap_or_default();
    ctx.author = proxy::run_git_capture_in(cfg, &["log", "-1", "--format=%an <%ae>", sha], root)
        .unwrap_or_default();
    ctx.committed_at = proxy::run_git_capture_in(cfg, &["log", "-1", "--format=%ct", sha], root)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(|| chrono::Utc::now().timestamp());

    let range = format!("{sha}~1..{sha}");
    if let Ok(stat) = proxy::run_git_capture_in(cfg, &["diff", "--shortstat", &range], root) {
        parse_shortstat(&stat, &mut ctx);
    }

    ctx
}

fn parse_shortstat(stat: &str, ctx: &mut GitContext) {
    for part in stat.split(',') {
        let part = part.trim();
        if part.contains("file") {
            if let Some(n) = part.split_whitespace().next() {
                ctx.files_changed = n.parse().unwrap_or(0);
            }
        } else if part.contains("insertion") {
            if let Some(n) = part.split_whitespace().next() {
                ctx.insertions = n.parse().unwrap_or(0);
            }
        } else if part.contains("deletion") {
            if let Some(n) = part.split_whitespace().next() {
                ctx.deletions = n.parse().unwrap_or(0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_shortstat() {
        let mut ctx = GitContext::default();

        parse_shortstat(
            " 3 files changed, 42 insertions(+), 10 deletions(-)",
            &mut ctx,
        );
        assert_eq!(ctx.files_changed, 3);
        assert_eq!(ctx.insertions, 42);
        assert_eq!(ctx.deletions, 10);
    }

    #[test]
    fn test_parse_shortstat_insert_only() {
        let mut ctx = GitContext::default();

        parse_shortstat(" 1 file changed, 5 insertions(+)", &mut ctx);
        assert_eq!(ctx.files_changed, 1);
        assert_eq!(ctx.insertions, 5);
        assert_eq!(ctx.deletions, 0);
    }
}
