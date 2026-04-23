use clap::{Parser, Subcommand};

use crate::config::Config;
use crate::git;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Tui,
    Agent,
    Json,
}

impl OutputMode {
    pub fn is_structured(self) -> bool {
        matches!(self, OutputMode::Agent | OutputMode::Json)
    }
}

/// oobo — git with memory
#[derive(Parser, Debug)]
#[command(
    name = "oobo",
    version,
    about = "Git with memory. Every commit tells you why it exists.",
    help_template = "\
{about-with-newline}
{usage-heading} {usage}

{options}
\x1b[1;4mAnchor:\x1b[0m  \x1b[2m(see your memory)\x1b[0m
  anchors, a   Enriched commit history with AI context
  blame        Per-line AI/human attribution

\x1b[1;4mRecall:\x1b[0m  \x1b[2m(find your memory)\x1b[0m
  search       Search sessions across all projects

\x1b[1;4mSettings:\x1b[0m  \x1b[2m(configure oobo)\x1b[0m
  settings     Declarative KV config
  enable       Start tracking this project
  disable      Stop tracking this project
  alias        Install/uninstall git→oobo shell alias

\x1b[1;4mLifecycle:\x1b[0m  \x1b[2m(onboard, repair, update)\x1b[0m
  setup        Onboard, repair, manage projects
  update       Self-update

\x1b[1;4mGit passthrough:\x1b[0m
  Any command not listed above is forwarded to git unchanged.
  Write operations (commit, push, merge) also capture AI context.

  oobo status              git status
  oobo commit -m \"fix\"     git commit + AI context capture
  oobo push origin main    git push + anchor sync

\x1b[2mUse --agent for compact agent output or --json for structured JSON.\x1b[0m
",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Compact output for AI agents (minimal, pipe-delimited)
    #[arg(long, global = true, conflicts_with = "json")]
    pub agent: bool,

    /// Structured JSON output for scripts and programmatic use
    #[arg(long, global = true, conflicts_with = "agent")]
    pub json: bool,

    /// Force pretty/TUI output even when auto-detection would pick agent mode
    #[arg(long, global = true, conflicts_with_all = ["agent", "json"])]
    pub interactive: bool,

    /// Raw args passed when invoked as a git alias (everything after `oobo`)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub git_args: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Show enriched commit history with anchor metadata
    #[command(
        display_order = 1,
        alias = "a",
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo anchors               Show recent commits with AI context\n  \
                       oobo a -n 20               Show last 20 (short alias)\n  \
                       oobo anchors --agent       Compact output\n  \
                       oobo anchors --json        Full JSON output"
    )]
    Anchors {
        /// Number of commits to show
        #[arg(short = 'n', long, default_value = "10")]
        limit: usize,
    },

    /// Show per-line AI/human attribution for a file
    #[command(
        display_order = 2,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo blame src/main.rs          Show AI attribution for file at HEAD\n  \
                       oobo blame src/main.rs abc123   Show attribution at a specific commit\n  \
                       oobo blame src/main.rs --json   JSON output"
    )]
    Blame {
        /// File path to show attribution for
        file: String,
        /// Commit hash (defaults to HEAD)
        commit: Option<String>,
    },

    /// Onboarding + repair wizard (projects, hooks, keys, alias)
    #[command(display_order = 10)]
    Setup {
        /// Accept defaults non-interactively (CI-safe)
        #[arg(long)]
        non_interactive: bool,
        /// Force a full reindex
        #[arg(long)]
        reindex: bool,
        /// Remove the git→oobo shell alias
        #[arg(long)]
        uninstall_alias: bool,
    },

    /// Manage the git→oobo shell alias [install, uninstall]
    #[command(
        display_order = 11,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo alias install     Alias git→oobo in your shell\n  \
                       oobo alias uninstall   Remove the alias"
    )]
    Alias {
        #[command(subcommand)]
        action: AliasAction,
    },

    /// Check for updates or self-update
    #[command(display_order = 20)]
    Update {
        /// Only check, don't install
        #[arg(long)]
        check: bool,
        /// Run post-update migrations (internal, called by the new binary after update)
        #[arg(long, hide = true)]
        post_update: bool,
    },

    /// Internal hook plumbing (called by agent tools, not typed by users)
    #[command(hide = true)]
    Hooks {
        #[command(subcommand)]
        action: HookAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum AliasAction {
    /// Add `alias git=oobo` to your shell RC file
    Install,
    /// Remove the git→oobo alias from your shell RC file
    Uninstall,
}

#[derive(Subcommand, Debug)]
pub enum HookAction {
    /// Handle an agent lifecycle event
    Agent {
        /// Event name: session-start, session-end, stop
        event: String,
        /// Which tool fired this hook (cursor, claude, gemini, etc.)
        #[arg(long)]
        tool: Option<String>,
    },
    /// Post-commit hook handler
    PostCommit {
        /// Extra args passed by git (ignored)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        _args: Vec<String>,
    },
    /// Pre-push hook handler
    PrePush {
        /// Remote name and URL passed by git (ignored)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        _args: Vec<String>,
    },
}

/// Reserved oobo verbs. Anything else at argv[1] is forwarded to `git` (passthrough).
const OOBO_SUBCOMMANDS: &[&str] = &[
    "anchors", "a", "blame", "setup", "alias", "update", "hooks",
];

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

    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
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

    let mode = if cli.json {
        OutputMode::Json
    } else if cli.agent {
        OutputMode::Agent
    } else {
        OutputMode::Tui
    };

    let result = match cli.command {
        Some(Command::Setup { .. }) => {
            crate::setup::run_setup().map_err(|e| e.to_string())?;
            Ok(0)
        }
        Some(Command::Alias { action }) => {
            crate::alias::run(action)?;
            Ok(0)
        }
        Some(Command::Update { check, post_update }) => {
            if post_update {
                crate::commands::update::run_post_update()?;
            } else {
                crate::commands::update::run(check)?;
            }
            Ok(0)
        }
        Some(Command::Anchors { limit }) => {
            crate::commands::anchors::run(&cfg, limit, mode)?;
            Ok(0)
        }
        Some(Command::Blame { file, commit }) => {
            crate::commands::blame::run(&cfg, &file, commit.as_deref(), mode)?;
            Ok(0)
        }
        Some(Command::Hooks { action }) => {
            match action {
                HookAction::Agent { event, tool } => {
                    let mut payload = String::new();
                    if let Err(e) =
                        std::io::Read::read_to_string(&mut std::io::stdin(), &mut payload)
                    {
                        eprintln!("oobo: warning: could not read agent payload from stdin: {e}");
                    }
                    if payload.trim().is_empty() {
                        payload = "{}".to_string();
                    }
                    // Debug: log hook invocations to diagnose missed sessions.
                    if let Some(home) = dirs::home_dir() {
                        let log_dir = home.join(".oobo/logs");
                        let _ = std::fs::create_dir_all(&log_dir);
                        let line = format!(
                            "{} event={} tool={:?} payload={}\n",
                            chrono::Utc::now().to_rfc3339(),
                            event,
                            tool,
                            payload.trim(),
                        );
                        let _ = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(log_dir.join("hooks-debug.log"))
                            .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
                    }
                    crate::hooks::handle_event(&event, &payload, tool.as_deref())
                        .map_err(|e| e.to_string())?;
                }
                HookAction::PostCommit { .. } => {
                    if let Some(root) = git::proxy::project_root(&cfg) {
                        if std::env::var("OOBO_INTERCEPTED").is_err() {
                            if let Err(e) = crate::git::interceptor::on_write_op(&cfg, &["commit"])
                            {
                                eprintln!("oobo: warning: {e}");
                            }
                        }
                        crate::hooks::state::cleanup_stale(&root, 86400);
                    }
                }
                HookAction::PrePush { .. } => {
                    if let Some(root) = git::proxy::project_root(&cfg) {
                        if crate::git::orphan::branch_exists(&root) {
                            if let Err(e) = crate::git::orphan::push(&root) {
                                eprintln!("oobo: warning: could not push anchors: {e}");
                            }
                        }
                        crate::git::orphan::retry_pending_pushes(&root);
                    }
                }
            }
            Ok(0)
        }
        None => {
            if cli.git_args.is_empty() {
                use clap::CommandFactory;
                Cli::command().print_help().ok();
                println!();
                Ok(0)
            } else {
                let git_args: Vec<&str> = cli.git_args.iter().map(|s| s.as_str()).collect();
                git::proxy::run_and_intercept(&cfg, &git_args)
            }
        }
    };

    result
}
