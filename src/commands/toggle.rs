//! `oobo enable` / `oobo disable` — per-project on/off toggles.
//!
//! Imperative verbs, NOT settings keys. The project folder is the source of
//! truth: `.oobo/config` exists for enabled projects, and
//! `[project].enabled = false` disables a project without deleting its config.

use crate::cli::OutputMode;
use crate::config::Config;

/// `oobo enable` — mark the current project as tracked.
pub fn enable(cfg: &Config, mode: OutputMode) -> Result<i32, String> {
    let (root, name) = match project_context(cfg) {
        Some(tuple) => tuple,
        None => {
            eprintln!(
                "error: not a git repository. cd into a repo first, or use 'anchor setup' to manage multiple projects."
            );
            return Ok(1);
        }
    };

    let project_id = crate::project::id_for_root(&root);
    let created_project_config = !crate::project_config::exists(&root);
    let was_enabled = crate::project_config::is_enabled(&root);

    if was_enabled {
        emit_already_enabled(&name, &project_id, &root, mode);
        return Ok(0);
    }

    crate::project_config::set_enabled(&root, &project_id, true)?;
    // Best-effort: install hooks (idempotent) + kick off a background reindex.
    let _ = crate::hooks::install::install_project_hooks(&root);
    spawn_background_index(&root, &project_id);

    emit_enabled(&name, &project_id, &root, created_project_config, mode);
    Ok(0)
}

/// `oobo disable` — mark the current project as not tracked.
pub fn disable(cfg: &Config, mode: OutputMode) -> Result<i32, String> {
    let (root, name) = match project_context(cfg) {
        Some(tuple) => tuple,
        None => {
            eprintln!(
                "error: not a git repository. cd into a repo first, or use 'anchor setup' to manage multiple projects."
            );
            return Ok(1);
        }
    };

    let project_id = crate::project::id_for_root(&root);

    let explicitly_disabled =
        crate::project_config::exists(&root) && !crate::project_config::is_enabled(&root);
    if explicitly_disabled {
        emit_already_disabled(&name, &project_id, mode);
        return Ok(0);
    }

    crate::project_config::set_enabled(&root, &project_id, false)?;

    emit_disabled(&name, &project_id, mode);
    Ok(0)
}

// ── helpers ────────────────────────────────────────────────────────────────

fn project_context(cfg: &Config) -> Option<(String, String)> {
    let root = crate::git::proxy::project_root(cfg)?;
    let name = std::path::Path::new(&root)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    Some((root, name))
}

fn spawn_background_index(_root: &str, _project_id: &str) {}

fn emit_enabled(name: &str, id: &str, path: &str, created_project_config: bool, mode: OutputMode) {
    match mode {
        OutputMode::Agent => println!("enabled {name}"),
        OutputMode::Json => {
            let json = serde_json::json!({
                "project": { "id": id, "name": name, "path": path },
                "enabled": true,
                "indexing": true,
                "project_config": {
                    "path": crate::project_config::path_for(path),
                    "created": created_project_config,
                },
            });
            crate::utils::print_json(&json);
        }
        OutputMode::Tui => {
            println!("anchor enabled for '{name}'. indexing sessions in the background.");
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
            println!("anchor is already enabled for '{name}'.");
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
                "anchor disabled for '{name}'. existing anchors retained. run 'anchor enable' to resume."
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
            println!("anchor is already disabled for '{name}'.");
        }
    }
}
