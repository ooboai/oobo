//! Best-effort auto-indexing triggered at the start of view/read commands.
//!
//! Staleness rule: if the project was last scanned more than
//! [`AUTO_INDEX_INTERVAL_SECS`] ago (or never), spawn a detached thread
//! to run [`crate::scanner::scan_project`]. The hot path NEVER blocks on
//! this — first invocation may show slightly stale data, and the next
//! invocation will see the freshly indexed sessions.
//!
//! Respects `OOBO_NO_AUTO_INDEX=1` (debug/CI), and gracefully no-ops for
//! disabled projects or when the DB cannot be opened.

use crate::config::Config;

/// Minimum seconds between background scans for a given project.
const AUTO_INDEX_INTERVAL_SECS: i64 = 300;

/// Called from the top of view-style commands. Non-blocking.
pub fn maybe_kick(cfg: &Config) {
    if std::env::var("OOBO_NO_AUTO_INDEX")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
    {
        return;
    }

    let Some(root) = crate::git::proxy::project_root(cfg) else {
        return;
    };
    let project_id = crate::paths::slug_from_path(&root);

    let Ok(db) = crate::db::Db::open() else {
        return;
    };

    let settings = db.get_project_settings(&project_id).unwrap_or_default();
    if settings.ignored {
        return;
    }

    // Ensure the row exists so we can track last_scanned_at.
    let _ = db.ensure_project(&project_id, &root);

    let needs_scan = match db.get_project_by_id(&project_id) {
        Ok(Some(p)) => {
            let now = chrono::Utc::now().timestamp();
            now - p.last_scanned_at > AUTO_INDEX_INTERVAL_SECS
        }
        _ => true,
    };
    if !needs_scan {
        return;
    }

    let root = root.clone();
    std::thread::spawn(move || {
        let cfg = crate::config::Config::load_or_default();
        if let Ok(db) = crate::db::Db::open() {
            let _ = crate::scanner::scan_project(&db, &cfg, &root);
        }
    });
}
