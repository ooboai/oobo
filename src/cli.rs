use clap::{Parser, Subcommand};

use crate::config::Config;
use crate::git;

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
  ship         Send AI context to dashboard
  sync         Import anchors from existing repos
  ignore       Stop tracking this repo
  unignore     Re-enable tracking

\x1b[1;4mGlobal:\x1b[0m
  setup        First-time configuration wizard
  projects     Browse and manage all projects
  stats        Token usage analytics and attribution
  card         Developer stats card
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

\x1b[2mEvery command supports --agent for structured JSON output.\x1b[0m
",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Structured JSON output for all commands (for AI agents and scripts)
    #[arg(long, global = true)]
    pub agent: bool,

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
                       oobo sessions --agent      JSON output\n  \
                       oobo sessions --all        All projects\n  \
                       oobo sessions list --agent  JSON (explicit subcommand)\n  \
                       oobo sessions show <id> --agent   Conversation as JSON\n  \
                       oobo sessions search auth --agent  Search as JSON\n  \
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
                       oobo anchors --agent       JSON output"
    )]
    Anchors {
        /// Number of commits to show
        #[arg(short = 'n', long, default_value = "10")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Share a session (redacted) -- save locally or upload
    #[command(
        display_order = 3,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo share <id>             Preview redacted session\n  \
                       oobo share <id> --out s.json  Save to file\n  \
                       oobo share <id> --agent       JSON output"
    )]
    Share {
        /// Session ID or prefix
        session_id: String,
        /// Write to file instead of uploading
        #[arg(long)]
        out: Option<String>,
    },

    /// Send AI context to the dashboard now
    #[command(display_order = 4)]
    Ship,

    /// Import anchors from existing repos
    #[command(
        display_order = 5,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo sync               Import anchors from orphan branch\n  \
                       oobo sync               Safe to run multiple times (idempotent)"
    )]
    Sync,

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

    // ── Global commands (work from anywhere) ─────────────────────────────
    /// First-time configuration wizard
    #[command(display_order = 10)]
    Setup,

    /// Browse and manage all projects
    #[command(
        display_order = 11,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo projects              Interactive TUI\n  \
                       oobo projects --agent      JSON output\n  \
                       oobo projects list --agent  JSON (explicit subcommand)\n  \
                       oobo projects show myapp --agent  Project details as JSON\n  \
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
                       oobo stats --agent                  JSON output\n  \
                       oobo stats --project myapp --agent   Per-project JSON\n  \
                       oobo stats --tool cursor --agent     Per-tool JSON\n  \
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
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Show stats since this date or duration (e.g. 7d, 30d, 2026-02-01)
        #[arg(long)]
        since: Option<String>,
    },

    /// Developer stats card (shareable, no private data)
    #[command(
        display_order = 13,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo card                  Generate card + save oobo-card.md\n  \
                       oobo card --out dev.md     Save to custom path\n  \
                       oobo card --agent          JSON output"
    )]
    Card {
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Save markdown to a custom file path (default: oobo-card.md)
        #[arg(long)]
        out: Option<String>,
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
                       oobo sources --agent        JSON output"
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
                       oobo inspect --agent     JSON output"
    )]
    Inspect {
        /// Auto-fix issues that can be repaired
        #[arg(long)]
        fix: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Check for updates or self-update
    #[command(display_order = 23)]
    Update {
        /// Only check, don't install
        #[arg(long)]
        check: bool,
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
    List {
        /// Output as JSON (non-interactive, agent-friendly)
        #[arg(long)]
        json: bool,
    },
    /// Show details for a specific project
    Show {
        /// Project name, slug, or path
        name: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
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
        /// Output as JSON (non-interactive, agent-friendly)
        #[arg(long)]
        json: bool,
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
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Search sessions by keyword (matches name, first message, and transcript)
    Search {
        /// Search query
        query: String,
        /// Search across all projects
        #[arg(long)]
        all: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
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
    "setup", "sessions", "alias", "dash", "ship", "projects", "stats", "scan", "index", "update",
    "sources", "auth", "agent", "version", "hooks", "anchors", "a", "share", "inspect", "sync",
    "ignore", "unignore", "card",
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

    let agent_mode = cli.agent;

    let result = match cli.command {
        Some(Command::Setup) => {
            crate::setup::run_setup().map_err(|e| e.to_string())?;
            Ok(0)
        }
        Some(Command::Sessions { action, all }) => {
            let resolved = match action {
                Some(SessionAction::List {
                    all: a,
                    json,
                    tool,
                    limit,
                }) => SessionAction::List {
                    all: a,
                    json: json || agent_mode,
                    tool,
                    limit,
                },
                Some(SessionAction::Show { id, json }) => SessionAction::Show {
                    id,
                    json: json || agent_mode,
                },
                Some(SessionAction::Search {
                    query,
                    all: a,
                    json,
                    limit,
                }) => SessionAction::Search {
                    query,
                    all: a,
                    json: json || agent_mode,
                    limit,
                },
                Some(other) => other,
                None => SessionAction::List {
                    all,
                    json: agent_mode,
                    tool: None,
                    limit: None,
                },
            };
            crate::commands::sessions::run(&cfg, resolved)?;
            Ok(0)
        }
        Some(Command::Alias { action }) => {
            crate::alias::run(action)?;
            Ok(0)
        }
        Some(Command::Dash) => {
            crate::commands::dash::run(&cfg, agent_mode);
            Ok(0)
        }
        Some(Command::Ship) => {
            crate::commands::ship::run(&cfg)?;
            Ok(0)
        }
        Some(Command::Projects { action }) => {
            let resolved = match action {
                Some(ProjectAction::List { json }) => ProjectAction::List {
                    json: json || agent_mode,
                },
                Some(ProjectAction::Show { name, json }) => ProjectAction::Show {
                    name,
                    json: json || agent_mode,
                },
                Some(other) => other,
                None => ProjectAction::List { json: agent_mode },
            };
            crate::commands::projects::run(resolved)?;
            Ok(0)
        }
        Some(Command::Stats {
            project,
            tool,
            json,
            since,
        }) => {
            crate::commands::stats::run(project, tool, json || agent_mode, since)?;
            Ok(0)
        }
        Some(Command::Scan { project, quiet }) => {
            crate::commands::scan::run(&cfg, project, quiet || agent_mode)?;
            Ok(0)
        }
        Some(Command::Index {
            project,
            force,
            bg,
            status,
        }) => {
            crate::commands::index::run(project, force, bg, status, agent_mode)?;
            Ok(0)
        }
        Some(Command::Sources) => {
            crate::commands::sources::run_cmd(agent_mode)?;
            Ok(0)
        }
        Some(Command::Auth { action }) => {
            crate::commands::auth::run(action)?;
            Ok(0)
        }
        Some(Command::Update { check }) => {
            crate::commands::update::run(check)?;
            Ok(0)
        }
        Some(Command::Agent) => {
            crate::commands::agent::run()?;
            Ok(0)
        }
        Some(Command::Share { session_id, out }) => {
            crate::commands::share::run(&cfg, &session_id, out, agent_mode)?;
            Ok(0)
        }
        Some(Command::Anchors { limit, json }) => {
            crate::commands::anchors::run(&cfg, limit, json || agent_mode)?;
            Ok(0)
        }
        Some(Command::Inspect { fix, json }) => {
            crate::commands::check::run(fix, json || agent_mode)?;
            Ok(0)
        }
        Some(Command::Sync) => {
            crate::commands::sync::run(&cfg)?;
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
        Some(Command::Card { json, out }) => {
            crate::commands::card::run(json || agent_mode, out)?;
            Ok(0)
        }
        Some(Command::Version) => {
            print_oobo_version(&cfg, agent_mode);
            Ok(0)
        }
        Some(Command::Hooks { action }) => {
            match action {
                HookAction::Agent { event } => {
                    let mut payload = String::new();
                    if let Err(e) =
                        std::io::Read::read_to_string(&mut std::io::stdin(), &mut payload)
                    {
                        eprintln!("oobo: warning: could not read agent payload from stdin: {e}");
                    }
                    if payload.trim().is_empty() {
                        payload = "{}".to_string();
                    }
                    crate::hooks::handle_event(&event, &payload).map_err(|e| e.to_string())?;
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

fn print_oobo_version(cfg: &Config, agent_mode: bool) {
    let version = env!("CARGO_PKG_VERSION");
    let git_version = git::proxy::run_git_capture(cfg, &["--version"])
        .unwrap_or_else(|_| "not found".to_string());
    let git_ver = git_version.trim_start_matches("git version ").trim();
    let db_path = crate::paths::oobo_db_path();
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    if agent_mode {
        let json = serde_json::json!({
            "oobo_version": version,
            "git_version": git_ver,
            "db_path": db_path.display().to_string(),
            "os": os,
            "arch": arch,
        });
        crate::utils::print_json(&json);
    } else {
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
