//! `oobo enable` / `oobo disable` — per-project on/off toggles.
//!
//! Imperative verbs, NOT settings keys. The project folder is the source of
//! truth: `.oobo/config` exists for enabled projects, and
//! `[project].enabled = false` disables a project without deleting its config.

use crate::cli::OutputMode;
use crate::config::Config;
use crate::error::CmdResult;

/// `oobo enable` — mark the current project as tracked.
pub fn enable(cfg: &Config, mode: OutputMode) -> CmdResult {
    let (root, name) = if let Some(tuple) = project_context(cfg) { tuple } else {
        eprintln!("oobo: not inside a git repository.");
        return Ok(1);
    };

    let project_id = crate::project::id_for_root(&root);
    let created_project_config = !crate::project_config::exists(&root);
    let was_enabled = crate::project_config::is_enabled(&root);

    if was_enabled {
        // Always refresh hooks to pick up new events added in upgrades.
        let _ = crate::hooks::install::install_project_hooks(&root);
        let _ = crate::hooks::install::install_all_agent_hooks();
        emit_already_enabled(&name, &project_id, &root, mode);
        return Ok(0);
    }

    crate::project_config::set_enabled(&root, &project_id, true)?;
    let _ = crate::hooks::install::install_project_hooks(&root);
    let _ = crate::hooks::install::install_all_agent_hooks();

    emit_enabled(&name, &project_id, &root, created_project_config, mode);
    Ok(0)
}

/// `oobo disable` — mark the current project as not tracked.
pub fn disable(cfg: &Config, mode: OutputMode) -> CmdResult {
    let (root, name) = if let Some(tuple) = project_context(cfg) { tuple } else {
        eprintln!("oobo: not inside a git repository.");
        return Ok(1);
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

fn emit_enabled(name: &str, id: &str, path: &str, created_project_config: bool, mode: OutputMode) {
    match mode {
        OutputMode::Agent => println!("enabled {name}"),
        OutputMode::Json => {
            let json = serde_json::json!({
                "project": { "id": id, "name": name, "path": path },
                "enabled": true,
                "project_config": {
                    "path": crate::project_config::path_for(path),
                    "created": created_project_config,
                },
            });
            crate::utils::print_json(&json);
        }
        OutputMode::Tui => {
            println!("oobo enabled for '{name}'. hooks installed.");
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
                "noop": true,
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
