use std::path::Path;

use crate::config::Config;
use crate::git::orphan;

/// Check if this is the first time oobo runs in this repo.
/// Shows a first-use notice, fetches remote anchors if available.
///
/// Called from the git interceptor on write ops. Uses the DB to
/// determine first-use (project not yet registered) — no filesystem
/// marker required.
pub fn check_first_use(cfg: &Config, project_root: &str) {
    let db = match crate::db::Db::open() {
        Ok(db) => db,
        Err(_) => return,
    };

    let id = crate::project::id_for_root(project_root);
    let already_known = db
        .get_project_by_id(&id)
        .ok()
        .flatten()
        .is_some()
        || db
            .get_project_by_path(project_root)
            .ok()
            .flatten()
            .is_some();

    if already_known {
        check_orphan_health(&db, project_root);
        return;
    }

    // Register the project so subsequent calls are no-ops.
    let _ = crate::project::ensure_stable(&db, project_root);

    if crate::git::detect::is_interactive() {
        let short = Path::new(project_root)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(project_root);
        eprintln!(
            "  \x1b[1moobo:\x1b[0m enriching commits in \x1b[36m{short}\x1b[0m with AI attribution — run \x1b[1moobo disable\x1b[0m to opt out"
        );
    }

    if cfg.is_ignored(project_root) || is_project_disabled(&db, &id) {
        return;
    }

    if let Err(e) = crate::hooks::install::install_project_hooks(project_root) {
        eprintln!("  \x1b[33m!\x1b[0m Could not install git hooks: {e}");
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

    match orphan::fetch_and_reconcile(project_root) {
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

fn is_project_disabled(db: &crate::db::Db, project_id: &str) -> bool {
    db.get_project_settings(project_id)
        .ok()
        .map(|s| s.ignored)
        .unwrap_or(false)
}

/// For known projects, check if the orphan branch is healthy. If
/// the DB has anchors but the branch is missing, attempt to rebuild
/// from the remote (silently) or notify the user once.
fn check_orphan_health(db: &crate::db::Db, project_root: &str) {
    if orphan::branch_exists(project_root) {
        return;
    }

    let id = crate::project::id_for_root(project_root);
    let state_key = format!("orphan_warned:{id}");
    if db.state_get(&state_key).is_some() {
        return;
    }

    // Try silent fetch from remote first.
    if orphan::remote_branch_exists(project_root) {
        if orphan::fetch_and_reconcile(project_root).is_ok() {
            return;
        }
    }

    // Has anchors in DB but no branch — warn once.
    let has_anchors = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM anchors a \
             JOIN sessions s ON s.id = a.session_id \
             WHERE s.project_id = ?1",
            rusqlite::params![&id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;

    if has_anchors && crate::git::detect::is_interactive() {
        eprintln!(
            "  \x1b[33moobo:\x1b[0m anchor branch missing for this project. \
             Run \x1b[1moobo setup --repair\x1b[0m to rebuild."
        );
        let _ = db.state_set(&state_key, "1");
    }
}
