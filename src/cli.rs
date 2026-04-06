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

/// oobo — git decorator for humans and agents
#[derive(Parser, Debug)]
#[command(
    name = "oobo",
    about = "Git decorator for humans and agents. Decorates git to enrich commits with AI context",
    help_template = "\
{about-with-newline}
{usage-heading} {usage}

{options}
\x1b[1;4mGit:\x1b[0m
  Any command not listed below passes through to git.
  Write operations (commit, push, merge) also capture AI context.

  oobo status              git status
  oobo commit -m \"fix\"     git commit + AI context capture
  oobo push origin main    git push + anchor sync

\x1b[1;4mProject:\x1b[0m  \x1b[2m(run inside a repo)\x1b[0m
  sessions     Browse AI chat sessions
  anchors, a   Enriched commit history with AI context
  share        Share a redacted session
  sync         Enable/disable backend sync
  ignore       Stop tracking this repo
  unignore     Re-enable tracking
  transparency Per-repo transcript transparency

\x1b[1;4mGlobal:\x1b[0m
  setup        First-time configuration wizard
  projects     Browse and manage all projects
  stats        Token usage analytics and attribution
  card         AI development infographic
  scan         Discover projects and sessions
  index        Compute token analytics
  sources      Data source status and coverage
  dash         Configuration overview
  auth         Configure API keys and remote server
  alias        Manage git→oobo shell alias
  agent        Print AI agent skill file
  inspect      Diagnose and auto-repair issues
  update       Check for updates or self-update
  version      Show version info

\x1b[2mUse --agent for compact output or --json for structured JSON.\x1b[0m
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

    /// Raw args passed when invoked as a git alias (everything after `oobo`)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub git_args: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    // ── Project commands (run inside a repo) ─────────────────────────────
    /// Browse AI chat sessions [list, show, search, export]
    #[command(
        display_order = 1,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo sessions              Interactive TUI\n  \
                       oobo sessions --agent      Compact output\n  \
                       oobo sessions --json       Full JSON output\n  \
                       oobo sessions --all        All projects\n  \
                       oobo sessions list --agent  Compact (explicit subcommand)\n  \
                       oobo sessions show <id> --json    Conversation as JSON\n  \
                       oobo sessions search auth --agent  Search results\n  \
                       oobo sessions export <id> --format md --out chat.md"
    )]
    Sessions {
        #[command(subcommand)]
        action: Option<SessionAction>,

        /// Show sessions from all projects (shorthand for `sessions list --all`)
        #[arg(long)]
        all: bool,
    },

    /// Show enriched commit history with anchor metadata
    #[command(
        display_order = 2,
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
        display_order = 3,
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

    /// Share a session (redacted) -- save locally or upload
    #[command(
        display_order = 3,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo share <id>             Preview redacted session\n  \
                       oobo share <id> --out s.json  Save to file\n  \
                       oobo share <id> --agent       Compact output"
    )]
    Share {
        /// Session ID or prefix
        session_id: String,
        /// Write to file instead of uploading
        #[arg(long)]
        out: Option<String>,
    },

    /// Enable or disable backend sync
    #[command(
        display_order = 5,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo sync               Show current sync status\n  \
                       oobo sync on            Enable auto-sync for this project\n  \
                       oobo sync off           Disable auto-sync for this project\n  \
                       oobo sync key <key>     Set a per-project API key\n  \
                       oobo sync --import      Import anchors from orphan branch"
    )]
    Sync {
        /// on, off, or key (omit to show current status)
        mode: Option<String>,
        /// Value for the key subcommand
        value: Option<String>,
        /// Import anchors from orphan branch into local DB
        #[arg(long)]
        import: bool,
    },

    /// Stop tracking this repo
    #[command(
        display_order = 6,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo ignore             Ignore the current repo\n  \
                       oobo ignore --list      Show all ignored repos"
    )]
    Ignore {
        /// Show all ignored repos
        #[arg(long)]
        list: bool,
    },

    /// Re-enable tracking for a previously ignored repo
    #[command(display_order = 7)]
    Unignore,

    /// AI contribution summary for a range of commits (PR/MR context)
    #[command(
        display_order = 9,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo pr                              Auto-detect base from CI env\n  \
                       oobo pr --base origin/main            Explicit base ref\n  \
                       oobo pr --base abc123 --head def456   Explicit range\n  \
                       oobo pr --json                        Full JSON output\n  \
                       oobo pr --agent                       Compact output"
    )]
    Pr {
        /// Base ref (auto-detected from CI env vars, falls back to origin/main)
        #[arg(long)]
        base: Option<String>,
        /// Head ref (default: HEAD)
        #[arg(long)]
        head: Option<String>,
    },

    /// Control per-repo transcript transparency [on, off, reset]
    #[command(
        display_order = 8,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo transparency          Show current setting\n  \
                       oobo transparency on        Sync redacted transcripts for this repo\n  \
                       oobo transparency off       Keep transcripts local for this repo\n  \
                       oobo transparency reset     Clear override, use global default\n  \
                       oobo transparency --list    Show all per-repo overrides"
    )]
    Transparency {
        /// on, off, or reset (omit to show current setting)
        mode: Option<String>,
        /// Show all repos with per-repo transparency overrides
        #[arg(long)]
        list: bool,
    },

    // ── Global commands (work from anywhere) ─────────────────────────────
    /// First-time configuration wizard
    #[command(display_order = 10)]
    Setup,

    /// Browse and manage all projects
    #[command(
        display_order = 11,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo projects              Interactive TUI\n  \
                       oobo projects --agent      Compact output\n  \
                       oobo projects list --json   Full JSON output\n  \
                       oobo projects show myapp --agent  Project summary\n  \
                       oobo projects forget myapp  Remove a project from tracking"
    )]
    Projects {
        #[command(subcommand)]
        action: Option<ProjectAction>,
    },

    /// Token usage analytics, AI code attribution, and productivity metrics
    #[command(
        display_order = 12,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo stats                         Global stats\n  \
                       oobo stats --agent                  Compact output\n  \
                       oobo stats --json                   Full JSON output\n  \
                       oobo stats --project myapp --agent   Per-project stats\n  \
                       oobo stats --since 30d              Last 30 days\n  \
                       oobo stats --since 2026-02-01       Since a date"
    )]
    Stats {
        /// Filter by project name or slug
        #[arg(long)]
        project: Option<String>,
        /// Filter by tool (cursor, claude, windsurf, etc.)
        #[arg(long)]
        tool: Option<String>,
        /// Show stats since this date or duration (e.g. 7d, 30d, 2026-02-01)
        #[arg(long)]
        since: Option<String>,
    },

    /// AI development infographic (shareable, no private data)
    #[command(
        display_order = 13,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo card                       Generate SVG infographic\n  \
                       oobo card --out dev.svg         Save to custom path\n  \
                       oobo card --format md            Markdown output\n  \
                       oobo card --format json          JSON output\n  \
                       oobo card --agent                Compact output\n  \
                       oobo card --json                 Full JSON output"
    )]
    Card {
        /// Save to a custom file path (default: oobo-card.png)
        #[arg(long)]
        out: Option<String>,
        /// Output format: png (default), svg, md, json
        #[arg(long, default_value = "png", value_parser = ["png", "svg", "md", "json"])]
        format: String,
    },

    /// Discover projects and sessions across all AI tools
    #[command(display_order = 15)]
    Scan {
        /// Scan a specific project path
        #[arg(long)]
        project: Option<String>,
        /// Suppress output
        #[arg(long)]
        quiet: bool,
    },

    /// Compute token counts and analytics for indexed sessions
    #[command(
        display_order = 16,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo index                     Index all sessions\n  \
                       oobo index --project myapp      Index a specific project\n  \
                       oobo index --force              Re-index already indexed sessions\n  \
                       oobo index --bg                 Run in background with notification\n  \
                       oobo index --status             Check background indexing progress"
    )]
    Index {
        /// Index only sessions for this project (name, slug, or path)
        #[arg(long)]
        project: Option<String>,
        /// Re-compute stats even for already indexed sessions
        #[arg(long)]
        force: bool,
        /// Run indexing in the background (returns immediately, notifies on completion)
        #[arg(long)]
        bg: bool,
        /// Check the status of a background indexing job
        #[arg(long)]
        status: bool,
    },

    /// Data source status and coverage for all tools
    #[command(
        display_order = 17,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo sources                Show all data sources and coverage\n  \
                       oobo sources --agent        Compact output\n  \
                       oobo sources --json         Full JSON output"
    )]
    Sources,

    /// Configuration overview
    #[command(display_order = 18)]
    Dash,

    /// Configure API keys and remote server
    #[command(
        display_order = 19,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo auth login              Log in to oobo.dev\n  \
                       oobo auth status             Show auth state\n  \
                       oobo auth anthropic <key>    Set Anthropic Admin API key\n  \
                       oobo auth openai <key>       Set OpenAI API key\n  \
                       oobo auth set-remote <url>   Self-hosted server"
    )]
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },

    /// Manage git→oobo shell alias [install, uninstall]
    #[command(
        display_order = 20,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo alias install     Alias git→oobo in your shell\n  \
                       oobo alias uninstall   Remove the alias"
    )]
    Alias {
        #[command(subcommand)]
        action: AliasAction,
    },

    /// Print AI agent skill file
    #[command(display_order = 21)]
    Agent,

    /// Diagnose and auto-repair common issues
    #[command(
        display_order = 22,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo inspect             Run diagnostics\n  \
                       oobo inspect --fix       Auto-repair what can be fixed\n  \
                       oobo inspect --agent     Compact output\n  \
                       oobo inspect --json      Full JSON output"
    )]
    Inspect {
        /// Auto-fix issues that can be repaired
        #[arg(long)]
        fix: bool,
    },

    /// Check for updates or self-update
    #[command(display_order = 23)]
    Update {
        /// Only check, don't install
        #[arg(long)]
        check: bool,
        /// Run post-update migrations (internal, called by the new binary after update)
        #[arg(long, hide = true)]
        post_update: bool,
    },

    /// Show oobo version, git version, and environment info
    #[command(display_order = 24)]
    Version,

    /// Internal hook plumbing (called by agent tools, not typed by users)
    #[command(hide = true)]
    Hooks {
        #[command(subcommand)]
        action: HookAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProjectAction {
    /// List all tracked projects
    List,
    /// Show details for a specific project
    Show {
        /// Project name, slug, or path
        name: String,
    },
    /// Remove a project from tracking
    Forget {
        /// Project name, slug, or path
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum SessionAction {
    /// List sessions for the current project
    List {
        /// Show sessions from all projects
        #[arg(long)]
        all: bool,
        /// Filter by tool (cursor, claude, gemini, etc.)
        #[arg(long)]
        tool: Option<String>,
        /// Max number of sessions to return (default: all)
        #[arg(long, short = 'n')]
        limit: Option<usize>,
    },
    /// Show a session's conversation
    Show {
        /// Session ID (prefix match supported)
        id: String,
    },
    /// Search sessions by keyword (matches name, first message, and transcript)
    Search {
        /// Search query
        query: String,
        /// Search across all projects
        #[arg(long)]
        all: bool,
        /// Max results (default: 20)
        #[arg(long, short = 'n', default_value = "20")]
        limit: usize,
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

#[derive(Subcommand, Debug)]
pub enum AuthAction {
    /// Log in to oobo.dev (or self-hosted server)
    Login {
        /// API key (prompted interactively if omitted)
        #[arg(long)]
        key: Option<String>,
    },
    /// Log out and remove stored credentials
    Logout,
    /// Show current auth status and remote server
    Status,
    /// Set custom remote server URL (for self-hosted / enterprise)
    SetRemote {
        /// Server URL (e.g. https://oobo.mycompany.com)
        url: String,
    },
    /// Set Anthropic Admin API key
    Anthropic {
        /// Admin API key (sk-ant-admin...)
        key: String,
    },
    /// Set GitHub Copilot org PAT
    Copilot {
        /// Personal access token with manage_billing:copilot scope
        token: String,
    },
    /// Set Windsurf/Codeium service key
    Windsurf {
        /// Service key with Analytics Read permission
        key: String,
    },
    /// Set OpenAI API key
    Openai {
        /// OpenAI API key
        key: String,
    },
    /// Set Google AI Studio API key (for Gemini usage data)
    Google {
        /// Google AI Studio API key
        key: String,
    },
    /// Show configured tool API keys (legacy alias for `status`)
    Show,
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

const OOBO_SUBCOMMANDS: &[&str] = &[
    "setup",
    "sessions",
    "alias",
    "dash",
    "projects",
    "stats",
    "scan",
    "index",
    "update",
    "sources",
    "auth",
    "agent",
    "version",
    "hooks",
    "anchors",
    "a",
    "blame",
    "share",
    "inspect",
    "pr",
    "sync",
    "ignore",
    "unignore",
    "transparency",
    "card",
];

fn is_oobo_subcommand(args: &[String]) -> bool {
    args.get(1)
        .map(|a| OOBO_SUBCOMMANDS.contains(&a.as_str()))
        .unwrap_or(false)
}

/// Determine what to do and dispatch.
pub fn route(mut cfg: Config) -> Result<i32, String> {
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
            if e.kind() == clap::error::ErrorKind::DisplayHelp {
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
        Some(Command::Setup) => {
            crate::setup::run_setup().map_err(|e| e.to_string())?;
            Ok(0)
        }
        Some(Command::Sessions { action, all }) => {
            let resolved = match action {
                Some(a) => a,
                None => SessionAction::List {
                    all,
                    tool: None,
                    limit: None,
                },
            };
            crate::commands::sessions::run(&cfg, resolved, mode)?;
            Ok(0)
        }
        Some(Command::Alias { action }) => {
            crate::alias::run(action)?;
            Ok(0)
        }
        Some(Command::Dash) => {
            crate::commands::dash::run(&cfg, mode);
            Ok(0)
        }
        Some(Command::Projects { action }) => {
            let resolved = match action {
                Some(a) => a,
                None => ProjectAction::List,
            };
            crate::commands::projects::run(resolved, mode)?;
            Ok(0)
        }
        Some(Command::Stats {
            project,
            tool,
            since,
        }) => {
            crate::commands::stats::run(project, tool, mode, since)?;
            Ok(0)
        }
        Some(Command::Scan { project, quiet }) => {
            crate::commands::scan::run(&cfg, project, quiet || mode.is_structured())?;
            Ok(0)
        }
        Some(Command::Index {
            project,
            force,
            bg,
            status,
        }) => {
            crate::commands::index::run(project, force, bg, status, mode.is_structured())?;
            Ok(0)
        }
        Some(Command::Sources) => {
            crate::commands::sources::run_cmd(mode)?;
            Ok(0)
        }
        Some(Command::Auth { action }) => {
            crate::commands::auth::run(action)?;
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
        Some(Command::Agent) => {
            crate::commands::agent::run()?;
            Ok(0)
        }
        Some(Command::Share { session_id, out }) => {
            crate::commands::share::run(&cfg, &session_id, out, mode)?;
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
        Some(Command::Pr { base, head }) => {
            crate::commands::pr::run(&cfg, base.as_deref(), head.as_deref(), mode)?;
            Ok(0)
        }
        Some(Command::Inspect { fix }) => {
            crate::commands::check::run(fix, mode)?;
            Ok(0)
        }
        Some(Command::Sync {
            mode,
            value,
            import,
        }) => {
            if import {
                crate::commands::sync::run_import(&cfg)?;
            } else {
                crate::commands::sync::run(&mut cfg, mode.as_deref(), value.as_deref())?;
            }
            Ok(0)
        }
        Some(Command::Ignore { list }) => {
            if list {
                crate::commands::ignore::run_list(&cfg);
            } else {
                crate::commands::ignore::run_ignore(&cfg)?;
            }
            Ok(0)
        }
        Some(Command::Unignore) => {
            crate::commands::ignore::run_unignore(&cfg)?;
            Ok(0)
        }
        Some(Command::Transparency { mode, list }) => {
            if list {
                crate::commands::transparency::run_list(&cfg);
            } else {
                crate::commands::transparency::run(&cfg, mode.as_deref())?;
            }
            Ok(0)
        }
        Some(Command::Card { out, format }) => {
            crate::commands::card::run(mode, out, &format)?;
            Ok(0)
        }
        Some(Command::Version) => {
            print_oobo_version(&cfg, mode);
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

fn print_oobo_version(cfg: &Config, mode: OutputMode) {
    let version = env!("CARGO_PKG_VERSION");
    let git_version = git::proxy::run_git_capture(cfg, &["--version"])
        .unwrap_or_else(|_| "not found".to_string());
    let git_ver = git_version.trim_start_matches("git version ").trim();
    let db_path = crate::paths::oobo_db_path();
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    match mode {
        OutputMode::Agent => {
            println!("version: {version}");
            println!("git: {git_ver}");
            println!("os: {os} {arch}");
        }
        OutputMode::Json => {
            let json = serde_json::json!({
                "oobo_version": version,
                "git_version": git_ver,
                "db_path": db_path.display().to_string(),
                "os": os,
                "arch": arch,
            });
            crate::utils::print_json(&json);
        }
        OutputMode::Tui => {
            println!("oobo {} ({})", version, env!("CARGO_PKG_HOMEPAGE"));
            println!("git:  {git_ver}");

            let db_size = std::fs::metadata(&db_path)
                .map(|m| {
                    let bytes = m.len();
                    if bytes >= 1_048_576 {
                        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
                    } else if bytes >= 1024 {
                        format!("{:.0} KB", bytes as f64 / 1024.0)
                    } else {
                        format!("{bytes} B")
                    }
                })
                .unwrap_or_else(|_| "not created".to_string());
            println!("db:   {} ({})", db_path.display(), db_size);
            println!("os:   {os} {arch}");
        }
    }
}
