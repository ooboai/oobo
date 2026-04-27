mod alias;
mod analytics;
mod attribution;
mod cli;
mod commands;
mod config;
mod core;
mod error;
mod git;
mod hooks;
#[allow(dead_code)]
mod notify;
mod paths;
mod project;
mod project_config;
mod redact;
mod remote;
mod session;
mod setup;
mod taps;
mod tools;
mod trace;
mod tui;
mod utils;

use std::process;

fn main() {
    let cfg = config::Config::load_or_default();

    if let Err(e) = ensure_oobo_dirs() {
        eprintln!("anchor: warning: {e}");
    }

    run_startup_tasks(&cfg);

    match cli::route(cfg) {
        Ok(code) => process::exit(code),
        Err(e) => {
            eprintln!("anchor: {e}");
            process::exit(1);
        }
    }
}

fn run_startup_tasks(_cfg: &config::Config) {
    use std::io::IsTerminal;
    let flag_path = paths::oobo_home().join("state").join("welcomed_v1");
    if std::io::stderr().is_terminal() && !flag_path.exists() {
        let _ = std::fs::create_dir_all(flag_path.parent().unwrap());
        let _ = std::fs::write(&flag_path, "1");
        let version = env!("CARGO_PKG_VERSION");
        eprintln!();
        eprintln!("  \x1b[1;36m  anchor {version}\x1b[0m");
        eprintln!();
    }
}

fn ensure_oobo_dirs() -> Result<(), String> {
    paths::ensure_dir(&paths::oobo_home())?;
    Ok(())
}
