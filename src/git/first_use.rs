use crate::config::Config;
use crate::git::orphan;

/// Check if this is the first time oobo runs in this repo.
/// Shows a first-use notice, fetches remote anchors if available.
///
/// Called from the git interceptor on write ops. Uses a marker file
/// in `.git/` to avoid repeating first-use work.
pub fn check_first_use(cfg: &Config, project_root: &str) {
    if cfg.is_ignored(project_root) || !crate::project_config::is_enabled(project_root) {
        return;
    }

    let git_dir = crate::git::detect::resolve_git_common_dir(project_root);
    let marker = git_dir.join("oobo-first-use-done");
    if marker.exists() {
        check_orphan_health(project_root);
        return;
    }

    let _ = std::fs::write(&marker, "1");

    if crate::git::detect::is_interactive() {
        let short = std::path::Path::new(project_root)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(project_root);
        eprintln!(
            "  \x1b[1moorbo:\x1b[0m enriching commits in \x1b[36m{short}\x1b[0m with AI attribution — run \x1b[1moorbo disable\x1b[0m to opt out"
        );
    }

    if let Err(e) = crate::hooks::install::install_project_hooks(project_root) {
        eprintln!("  \x1b[33m!\x1b[0m Could not install git hooks: {e}");
    }

    if orphan::branch_exists(project_root) {
        return;
    }

    if !orphan::remote_branch_exists(project_root) {
        return;
    }

    if crate::git::detect::is_interactive() {
        eprintln!("  This repo has anchor metadata on the remote. Pulling…");
    }

    match orphan::fetch_and_reconcile(project_root) {
        Ok(()) => {
            if crate::git::detect::is_interactive() {
                eprintln!("  \x1b[32m✓\x1b[0m Anchor metadata pulled.");
            }
        }
        Err(e) => {
            eprintln!("  \x1b[33m!\x1b[0m Could not fetch anchor data: {e}");
        }
    }
}

fn check_orphan_health(project_root: &str) {
    if orphan::branch_exists(project_root) {
        return;
    }

    let git_dir = crate::git::detect::resolve_git_common_dir(project_root);
    let flag = git_dir.join("oobo-orphan-warned");
    if flag.exists() {
        return;
    }

    if orphan::remote_branch_exists(project_root)
        && orphan::fetch_and_reconcile(project_root).is_ok()
    {
        return;
    }

    if crate::git::detect::is_interactive() {
        eprintln!(
            "  \x1b[33moorbo:\x1b[0m anchor branch missing for this project. \
             Run \x1b[1moorbo setup --repair\x1b[0m to rebuild."
        );
        let _ = std::fs::write(&flag, "1");
    }
}
