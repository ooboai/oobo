use std::path::Path;

use crate::config::Config;
use crate::git::orphan;

const MARKER_FILE: &str = ".git/oobo-initialized";

/// Check if this is the first time oobo runs in this repo.
/// Shows a first-use notice, fetches remote anchors if available.
///
/// Called from the git interceptor on write ops, but only
/// does work once per repo (writes a marker file).
pub fn check_first_use(cfg: &Config, project_root: &str) {
    let marker = Path::new(project_root).join(MARKER_FILE);
    if marker.exists() {
        return;
    }

    let _ = std::fs::write(&marker, "");

    // First-use notice so the user knows oobo is active here.
    if crate::git::detect::is_interactive() {
        let short = Path::new(project_root)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(project_root);
        eprintln!(
            "  \x1b[1moobo:\x1b[0m enriching commits in \x1b[36m{short}\x1b[0m with AI attribution — run \x1b[1moobo ignore\x1b[0m to opt out"
        );
    }

    if cfg.is_ignored(project_root) || is_project_ignored_db(project_root) {
        return;
    }

    if orphan::branch_exists(project_root) {
        crate::commands::sync::auto_hydrate(project_root);
        return;
    }

    if !orphan::remote_branch_exists(project_root) {
        return;
    }

    if crate::git::detect::is_interactive() {
        eprintln!("  This repo has anchor metadata on the remote. Pulling…");
    }

    match orphan::fetch(project_root) {
        Ok(()) => {
            if crate::git::detect::is_interactive() {
                eprintln!("  \x1b[32m✓\x1b[0m Anchor metadata pulled.");
            }
            crate::commands::sync::auto_hydrate(project_root);
        }
        Err(e) => {
            eprintln!("  \x1b[33m!\x1b[0m Could not fetch oobo data: {e}");
        }
    }
}

fn is_project_ignored_db(project_root: &str) -> bool {
    crate::db::Db::open()
        .ok()
        .and_then(|db| db.get_project_settings_by_path(project_root).ok())
        .map(|s| s.ignored)
        .unwrap_or(false)
}
