mod alias;
mod analytics;
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
mod tools;
mod tui;
mod utils;

use std::process;

fn main() {
    let cfg = config::Config::load_or_default();

    if let Err(e) = ensure_oobo_dirs() {
        eprintln!("oobo: warning: {e}");
    }

    match cli::route(cfg) {
        Ok(code) => process::exit(code),
        Err(e) => {
            eprintln!("oobo: {e}");
            process::exit(1);
        }
    }
}

fn ensure_oobo_dirs() -> Result<(), String> {
    paths::ensure_dir(&paths::oobo_home())?;
    paths::ensure_dir(&paths::oobo_db_dir())?;
    paths::ensure_dir(&paths::oobo_projects_dir())?;
    Ok(())
}
