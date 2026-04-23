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

    // Opportunistic one-time backfill after a schema bump. Migration
    // v12 arms a `backfill_pending` flag; the first run that sees it
    // rebuilds turns + contributions from native transcripts, then
    // clears the flag. Everyone gets the new token accounting on
    // their next invocation without typing a command.
    run_opportunistic_backfill(&cfg);

    match cli::route(cfg) {
        Ok(code) => process::exit(code),
        Err(e) => {
            eprintln!("oobo: {e}");
            process::exit(1);
        }
    }
}

fn run_opportunistic_backfill(cfg: &config::Config) {
    // Everything here is best-effort: a failed rebuild must not
    // block whatever the user actually typed. Surface a single
    // friendly line when we did work, stay silent otherwise.
    let mut db = match db::Db::open() {
        Ok(db) => db,
        Err(_) => return,
    };
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
}

fn ensure_oobo_dirs() -> Result<(), String> {
    paths::ensure_dir(&paths::oobo_home())?;
    paths::ensure_dir(&paths::oobo_db_dir())?;
    paths::ensure_dir(&paths::oobo_projects_dir())?;
    Ok(())
}
