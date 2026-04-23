//! `oobo enable` / `oobo disable` — per-project on/off toggles.
//!
//! Imperative verbs, NOT settings keys. Stored in the `project_settings`
//! DB row as `ignored: bool` (inverted: `ignored = true` ⇒ disabled).

use crate::cli::OutputMode;
use crate::config::Config;
use crate::db::Db;

/// `oobo enable` — mark the current project as tracked.
pub fn enable(cfg: &Config, mode: OutputMode) -> Result<i32, String> {
    let (root, project_id, name) = match project_context(cfg) {
        Some(tuple) => tuple,
        None => {
            eprintln!(
                "error: not a git repository. cd into a repo first, or use 'oobo setup' to manage multiple projects."
            );
            return Ok(1);
        }
    };

    let db = Db::open()?;
    let project_id = crate::project::ensure_stable(&db, &root)?;
    let mut settings = db.get_project_settings(&project_id).unwrap_or_default();

    let was_enabled = !settings.ignored;
    if was_enabled {
        emit_already_enabled(&name, &project_id, &root, mode);
        return Ok(0);
    }

    settings.ignored = false;
    db.set_project_settings(&project_id, &settings)?;

    // Best-effort: install hooks (idempotent) + kick off a background reindex.
    let _ = crate::hooks::install::install_project_hooks(&root);
    spawn_background_index(&root, &project_id);

    emit_enabled(&name, &project_id, &root, mode);
    Ok(0)
}

/// `oobo disable` — mark the current project as not tracked.
pub fn disable(cfg: &Config, mode: OutputMode) -> Result<i32, String> {
    let (root, _legacy_id, name) = match project_context(cfg) {
        Some(tuple) => tuple,
        None => {
            eprintln!(
                "error: not a git repository. cd into a repo first, or use 'oobo setup' to manage multiple projects."
            );
            return Ok(1);
        }
    };

    let db = Db::open()?;
    let project_id = crate::project::ensure_stable(&db, &root)?;
    let mut settings = db.get_project_settings(&project_id).unwrap_or_default();

    if settings.ignored {
        emit_already_disabled(&name, &project_id, mode);
        return Ok(0);
    }

    settings.ignored = true;
    db.set_project_settings(&project_id, &settings)?;

    emit_disabled(&name, &project_id, mode);
    Ok(0)
}

// ── helpers ────────────────────────────────────────────────────────────────

fn project_context(cfg: &Config) -> Option<(String, String, String)> {
    let root = crate::git::proxy::project_root(cfg)?;
    let project_id = crate::project::id_for_root(&root);
    let name = std::path::Path::new(&root)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    Some((root, project_id, name))
}

fn spawn_background_index(root: &str, _project_id: &str) {
    let root = root.to_string();
    std::thread::spawn(move || {
        let cfg = crate::config::Config::load_or_default();
        if let Ok(db) = crate::db::Db::open() {
            let _ = crate::scanner::scan_project(&db, &cfg, &root);
        }
    });
}

fn emit_enabled(name: &str, id: &str, path: &str, mode: OutputMode) {
    match mode {
        OutputMode::Agent => println!("enabled {name}"),
        OutputMode::Json => {
            let json = serde_json::json!({
                "project": { "id": id, "name": name, "path": path },
                "enabled": true,
                "indexing": true,
            });
            crate::utils::print_json(&json);
        }
        OutputMode::Tui => {
            println!("oobo enabled for '{name}'. indexing sessions in the background.");
        }
    }
}

fn emit_already_enabled(name: &str, id: &str, path: &str, mode: OutputMode) {
    match mode {
        OutputMode::Agent => println!("enabled {name} noop"),
        OutputMode::Json => {
            let json = serde_json::json!({
                "project": { "id": id, "name": name, "path": path },
                "enabled": true,
                "indexing": false,
            });
            crate::utils::print_json(&json);
        }
        OutputMode::Tui => {
            println!("oobo is already enabled for '{name}'.");
        }
    }
}

fn emit_disabled(name: &str, id: &str, mode: OutputMode) {
    match mode {
        OutputMode::Agent => println!("disabled {name}"),
        OutputMode::Json => {
            let json = serde_json::json!({
                "project": { "id": id, "name": name },
                "enabled": false,
            });
            crate::utils::print_json(&json);
        }
        OutputMode::Tui => {
            println!(
                "oobo disabled for '{name}'. existing anchors retained. run 'oobo enable' to resume."
            );
        }
    }
}

fn emit_already_disabled(name: &str, id: &str, mode: OutputMode) {
    match mode {
        OutputMode::Agent => println!("disabled {name} noop"),
        OutputMode::Json => {
            let json = serde_json::json!({
                "project": { "id": id, "name": name },
                "enabled": false,
            });
            crate::utils::print_json(&json);
        }
        OutputMode::Tui => {
            println!("oobo is already disabled for '{name}'.");
        }
    }
}
