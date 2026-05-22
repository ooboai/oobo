use crate::config::Config;

use super::proxy;

#[derive(Debug, Clone, Default)]
pub(in crate::git) struct GitContext {
    pub commit_hash: String,
    pub commit_message: String,
    pub author: String,
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
}

pub(in crate::git) fn collect_git_context(cfg: &Config, op: &str) -> GitContext {
    let mut ctx = GitContext::default();

    if op == "commit" || op == "merge" || op == "cherry-pick" || op == "revert" {
        ctx.commit_hash = proxy::run_git_capture(cfg, &["rev-parse", "HEAD"]).unwrap_or_default();
        ctx.commit_message =
            proxy::run_git_capture(cfg, &["log", "-1", "--format=%s"]).unwrap_or_default();
        ctx.author =
            proxy::run_git_capture(cfg, &["log", "-1", "--format=%an <%ae>"]).unwrap_or_default();

        if let Ok(stat) = proxy::run_git_capture(cfg, &["diff", "--shortstat", "HEAD~1", "HEAD"]) {
            parse_shortstat(&stat, &mut ctx);
        }
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
