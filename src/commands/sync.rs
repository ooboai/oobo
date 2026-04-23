//! Internal sync helpers used by the git interceptor and first-use flows.
//!
//! There is no user-facing `oobo sync` command in v1.0; the same behavior is
//! configured declaratively via `oobo settings` and per-project via
//! `oobo enable` / `oobo disable`.

use crate::config::Config;

const HYDRATION_INTERVAL_SECS: i64 = 3600;

/// Check the per-project sync override. Returns `None` if no override exists
/// (fall back to global config).
pub fn resolve_project_sync(cfg: &Config) -> Option<bool> {
    let project_root = crate::git::proxy::project_root(cfg)?;
    let db = crate::db::Db::open().ok()?;
    let slug = crate::paths::slug_from_path(&project_root);
    let settings = db.get_project_settings(&slug).ok()?;
    settings.sync
}

/// Resolve per-project API key, falling back to global config.
pub fn resolve_api_key(cfg: &Config) -> String {
    if let Some(project_root) = crate::git::proxy::project_root(cfg) {
        if let Ok(db) = crate::db::Db::open() {
            let slug = crate::paths::slug_from_path(&project_root);
            if let Ok(settings) = db.get_project_settings(&slug) {
                if let Some(ref key) = settings.api_key {
                    if !key.is_empty() {
                        return key.clone();
                    }
                }
            }
        }
    }
    cfg.server.api_key.clone()
}

/// Auto-sync: called from the interceptor or any project-aware command.
/// Non-blocking — only runs if overdue. Returns quietly on any error.
pub fn auto_hydrate(project_root: &str) {
    let db = match crate::db::Db::open() {
        Ok(db) => db,
        Err(_) => return,
    };

    if !db.needs_hydration(project_root, HYDRATION_INTERVAL_SECS) {
        return;
    }

    if !crate::git::orphan::branch_exists(project_root) {
        let _ = crate::git::orphan::fetch_and_reconcile(project_root);
        if !crate::git::orphan::branch_exists(project_root) {
            let _ = db.mark_hydrated(project_root, 0);
            return;
        }
    }

    match crate::git::orphan::hydrate_from_branch(project_root, &db) {
        Ok(n) => {
            let _ = db.mark_hydrated(project_root, n);
            if n > 0 {
                eprintln!("oobo: synced {n} anchor(s) from orphan branch");
            }
        }
        Err(e) => eprintln!("oobo: warning: hydration failed: {e}"),
    }
}
