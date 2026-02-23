mod aider;
mod alias;
mod claude;
mod cli;
mod codex;
mod commands;
mod config;
mod continue_dev;
mod copilot;
mod cursor;
mod git;
mod server;
mod session;
mod setup;
mod trae;
mod tui;
mod vscode_fork;
mod windsurf;
mod zed;

use std::process;

fn main() {
    let cfg = config::Config::load_or_default();

    match cli::route(cfg) {
        Ok(code) => process::exit(code),
        Err(e) => {
            eprintln!("oobo: {e}");
            process::exit(1);
        }
    }
}
