mod alias;
mod analytics;
mod attribution;
mod cli;
mod commands;
mod config;
mod core;
mod db;
mod error;
mod git;
mod hooks;
mod notify;
mod paths;
mod project;
mod redact;
mod remote;
mod scanner;
mod session;
mod setup;
mod taps;
mod tools;
mod tui;
mod utils;

use std::process;

fn main() {
    let cfg = config::Config::load_or_default();

    if let Err(e) = ensure_oobo_dirs() {
        eprintln!("oobo: warning: {e}");
    }

    run_startup_tasks(&cfg);

    match cli::route(cfg) {
        Ok(code) => process::exit(code),
        Err(e) => {
            eprintln!("oobo: {e}");
            process::exit(1);
        }
    }
}

/// All one-time startup tasks that need DB access, consolidated into a
/// single `Db::open()` call to avoid lock contention. Every check here
/// is idempotent and guarded by a flag — the common path is: open DB,
/// check 3 flags (all absent), return immediately.
fn run_startup_tasks(cfg: &config::Config) {
    let mut db = match db::Db::open() {
        Ok(db) => db,
        Err(_) => return,
    };

    // 1. Opportunistic backfill (v12 flag).
    if let Some(report) = attribution::auto_backfill::backfill_if_pending(&mut db, cfg) {
        eprintln!(
            "oobo: one-time data rebuild complete ({} projects, {} sessions, {} turns, {} subagents inferred)",
            report.projects_succeeded,
            report.sessions_scanned,
            report.turns_emitted,
            report.subagents_inferred,
        );
        for (pid, err) in &report.failures {
            eprintln!("  warn: backfill failed for {pid}: {err}");
        }
    }

    // 2. Legacy drain (v13 flag).
    if db.state_get("drain_legacy_pending").as_deref() == Some("1") {
        let mut drained = 0usize;
        let mut markers_removed = 0usize;

        if let Ok(projects) = db.list_projects() {
            for p in &projects {
                let root = &p.path;
                let git_dir = git::detect::resolve_git_dir(root);
                if !git_dir.exists() {
                    continue;
                }
                let legacy_dir = git_dir.join("oobo-sessions");
                if legacy_dir.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(&legacy_dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                                continue;
                            }
                            if let Ok(content) = std::fs::read_to_string(&path) {
                                if let Ok(state) =
                                    serde_json::from_str::<hooks::state::ActiveSession>(&content)
                                {
                                    let sid = &state.session_id;
                                    let _ = hooks::store::write(root, sid, &state);
                                    drained += 1;
                                }
                            }
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                    let _ = std::fs::remove_dir(&legacy_dir);
                }
                let marker = git_dir.join("oobo-initialized");
                if marker.exists() {
                    let _ = std::fs::remove_file(&marker);
                    markers_removed += 1;
                }
            }
        }

        let _ = db.state_clear("drain_legacy_pending");
        if drained > 0 || markers_removed > 0 {
            eprintln!(
                "oobo: migrated {drained} legacy session file(s), \
                 removed {markers_removed} marker file(s)"
            );
        }
    }

    // 3. Welcome banner (once per v1 upgrade, TTY only).
    use std::io::IsTerminal;
    if std::io::stderr().is_terminal() && db.state_get("welcomed_v1").is_none() {
        let _ = db.state_set("welcomed_v1", "1");
        let version = env!("CARGO_PKG_VERSION");
        eprintln!();
        eprintln!("  \x1b[1;36m  oobo {version}\x1b[0m — The Sharpening");
        eprintln!();
        eprintln!("  What's new:");
        eprintln!("    \x1b[32m•\x1b[0m Per-turn token accounting (no more cumulative inflation)");
        eprintln!("    \x1b[32m•\x1b[0m Subagent detection & hierarchy tracking");
        eprintln!("    \x1b[32m•\x1b[0m Full tap coverage: Claude, Cursor, Codex, OpenCode");
        eprintln!("    \x1b[32m•\x1b[0m Zero per-repo filesystem state (everything in ~/.oobo/db)");
        eprintln!("    \x1b[32m•\x1b[0m Unified TUI with project-scoped views");
        eprintln!();
        eprintln!("  Run \x1b[1moobo\x1b[0m in any project to explore, or \x1b[1moobo --help\x1b[0m for all commands.");
        eprintln!();
    }
}

fn ensure_oobo_dirs() -> Result<(), String> {
    paths::ensure_dir(&paths::oobo_home())?;
    paths::ensure_dir(&paths::oobo_db_dir())?;
    paths::ensure_dir(&paths::oobo_projects_dir())?;
    Ok(())
}
