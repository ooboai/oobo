use clap::{Parser, Subcommand};

use crate::config::Config;
use crate::git;

/// oobo — git wrapper with AI context
#[derive(Parser, Debug)]
#[command(
    name = "oobo",
    version,
    about = "Git wrapper that captures AI chat context alongside your commits",
    long_about = "Git wrapper that captures AI chat context alongside your commits.\n\
                   Supports: Cursor, Claude Code, Windsurf, Aider, Continue.dev, Copilot Chat, Zed, Trae, Codex CLI.\n\n\
                   Any command not listed below is passed straight through to git.\n\
                   Write operations (commit, push, merge, …) also capture AI context.",
    after_help = "\x1b[1mExamples:\x1b[0m\n  \
                   oobo status              Run git status\n  \
                   oobo commit -m \"fix\"      Commit + capture AI context\n  \
                   oobo log --oneline -5    Run git log\n  \
                   oobo sessions            List AI chat sessions\n  \
                   oobo sessions show 2c97  Show a session's conversation\n  \
                   oobo dash                Show oobo config & connection info",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Raw args passed when invoked as a git alias (everything after `oobo`)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub git_args: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run first-time configuration wizard
    Setup,

    /// Browse AI chat sessions [list, show, export]
    #[command(after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo sessions              List sessions for this project\n  \
                       oobo sessions --all        List sessions across all projects\n  \
                       oobo sessions show 2c97    Show a conversation (prefix match)\n  \
                       oobo sessions show 2c97 --json\n  \
                       oobo sessions export 2c97 --format md --out chat.md")]
    Sessions {
        #[command(subcommand)]
        action: Option<SessionAction>,

        /// Show sessions from all projects (shorthand for `sessions list --all`)
        #[arg(long)]
        all: bool,
    },

    /// Manage the git→oobo shell alias [install, uninstall]
    #[command(after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo alias install     Alias git→oobo in your shell\n  \
                       oobo alias uninstall   Remove the alias")]
    Alias {
        #[command(subcommand)]
        action: AliasAction,
    },

    /// Show oobo config, AI sessions, and server connection
    Dash,

    /// Send AI context to the dashboard now
    Ship,
}

#[derive(Subcommand, Debug)]
pub enum SessionAction {
    /// List sessions for the current project
    List {
        /// Show sessions from all projects
        #[arg(long)]
        all: bool,
    },
    /// Show a session's conversation
    Show {
        /// Session ID (prefix match supported)
        id: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Export a session to a file
    Export {
        /// Session ID (prefix match supported)
        id: String,
        /// Output format (md or json)
        #[arg(long, default_value = "md")]
        format: String,
        /// Output file path (prints to stdout if omitted)
        #[arg(long)]
        out: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum AliasAction {
    /// Add `alias git=oobo` to your shell RC file
    Install,
    /// Remove the git→oobo alias from your shell RC file
    Uninstall,
}

const OOBO_SUBCOMMANDS: &[&str] = &["setup", "sessions", "alias", "dash", "ship"];

fn is_oobo_subcommand(args: &[String]) -> bool {
    args.get(1)
        .map(|a| OOBO_SUBCOMMANDS.contains(&a.as_str()))
        .unwrap_or(false)
}

/// Determine what to do and dispatch.
pub fn route(cfg: Config) -> Result<i32, String> {
    let raw_args: Vec<String> = std::env::args().collect();

    // If invoked as `git` (via alias), treat everything as git args
    let invoked_as_git = raw_args
        .first()
        .map(|a| {
            let name = std::path::Path::new(a)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(a);
            name == "git"
        })
        .unwrap_or(false);

    if invoked_as_git {
        let git_args: Vec<&str> = raw_args.iter().skip(1).map(|s| s.as_str()).collect();
        return git::proxy::run_and_intercept(&cfg, &git_args);
    }

    // Try parsing as oobo subcommand
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            // Help and version requests should always be handled by clap
            if e.kind() == clap::error::ErrorKind::DisplayHelp
                || e.kind() == clap::error::ErrorKind::DisplayVersion
            {
                e.exit();
            }
            // If the first arg is one of our subcommands, show clap's error
            // (e.g. missing required arg) instead of passing to git
            if is_oobo_subcommand(&raw_args) {
                e.exit();
            }
            // For other parse failures with args present, treat as git passthrough
            if raw_args.len() > 1 {
                let git_args: Vec<&str> = raw_args.iter().skip(1).map(|s| s.as_str()).collect();
                return git::proxy::run_and_intercept(&cfg, &git_args);
            }
            e.exit();
        }
    };

    match cli.command {
        Some(Command::Setup) => {
            crate::setup::run_setup().map_err(|e| e.to_string())?;
            Ok(0)
        }
        Some(Command::Sessions { action, all }) => {
            let resolved = action.unwrap_or(SessionAction::List { all });
            crate::commands::sessions::run(&cfg, resolved)?;
            Ok(0)
        }
        Some(Command::Alias { action }) => {
            crate::alias::run(action)?;
            Ok(0)
        }
        Some(Command::Dash) => {
            crate::commands::dash::run(&cfg);
            Ok(0)
        }
        Some(Command::Ship) => {
            crate::commands::ship::run(&cfg)?;
            Ok(0)
        }
        None => {
            if cli.git_args.is_empty() {
                // No subcommand, no git args → show help
                use clap::CommandFactory;
                Cli::command().print_help().ok();
                println!();
                Ok(0)
            } else {
                let git_args: Vec<&str> = cli.git_args.iter().map(|s| s.as_str()).collect();
                git::proxy::run_and_intercept(&cfg, &git_args)
            }
        }
    }
}
