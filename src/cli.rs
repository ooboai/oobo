use clap::{Parser, Subcommand};

use crate::config::Config;
use crate::error::CmdResult;
use crate::git;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Tui,
    Agent,
    Json,
}

/// oobo — developer memory for humans and agents
#[derive(Parser, Debug)]
#[command(
    name = "oobo",
    version,
    about = "oobo — developer memory for humans and agents.",
    help_template = "\
{about}

Usage: oobo [OPTIONS] [COMMAND]

Commands (require a git repository):
  anchors      Memory feed — list anchors and active sessions
  anchor       Inspect a single anchor (show, blame)
  goto         Travel to a turn or commit (auto-stashes)
  back         Return to where you were before goto
  blame        Per-line AI/human attribution
  search       Find past sessions and anchors
  enable       Start tracking this project
  disable      Stop tracking this project

Commands (work anywhere):
  setup        Onboarding wizard — install hooks, configure tools
  settings     Show / set / unset configuration
  help         Built-in documentation (oobo help <topic>)
  update       Self-update to the latest version

Without a subcommand, oobo shows the memory feed for the current project.

Options:
  -n, --limit <N>    Max items (default 50)
  --since <WHEN>     Time filter (e.g. 24h, 7d, ISO-8601)
  --tool <NAME>      Filter by tool (cursor, claude, gemini...)
  --agent            Minimal plain-text output (token-efficient)
  --json             Full structured JSON output
  --interactive      Force TUI even when auto-detection would not
  -h, --help         Print help
  -V, --version      Print version

Run `oobo <command> --help` for details on any command.
",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Compact, token-efficient plain-text output for LLMs and scripts
    #[arg(long, global = true, conflicts_with = "json")]
    pub agent: bool,

    /// Full-fidelity structured JSON (parseable by jq)
    #[arg(long, global = true, conflicts_with = "agent")]
    pub json: bool,

    /// Force pretty/TUI output even when auto-detection would pick agent mode
    #[arg(long, global = true, conflicts_with_all = ["agent", "json"])]
    pub interactive: bool,

    /// Max items to list (default 50)
    #[arg(short = 'n', long, default_value_t = 50, global = true)]
    pub limit: usize,

    /// Only items at/after this point (e.g. 24h, 7d, ISO-8601)
    #[arg(long, global = true)]
    pub since: Option<String>,

    /// Filter by tool name (case-insensitive)
    #[arg(long, global = true)]
    pub tool: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Memory feed — list anchors and active sessions
    #[command(
        display_order = 1,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo anchors                  Memory feed (TUI in terminal)\n  \
                       oobo anchors --agent          Token-efficient listing\n  \
                       oobo anchors --json           Full JSON output\n  \
                       oobo anchors --since 7d       Last 7 days"
    )]
    Anchors {},

    /// Operate on a single anchor (show, blame)
    #[command(
        display_order = 2,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo anchor show a1b2c3d      Drill into a commit\n  \
                       oobo anchor show a1b2c3d --json  Full JSON output"
    )]
    Anchor {
        #[command(subcommand)]
        action: AnchorAction,
    },

    /// Travel to a turn or commit (auto-stashes dirty changes)
    #[command(
        display_order = 3,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo goto t123abc     Load a turn snapshot\n  \
                       oobo goto abc123      Load a commit\n  \
                       oobo back             Return to where you were"
    )]
    Goto {
        /// Turn ID or commit SHA (full or unambiguous prefix)
        target: String,
        /// Don't auto-stash dirty changes; fail instead.
        #[arg(long)]
        no_stash: bool,
    },

    /// Return to where you were before `goto` (restores stash if one was created)
    #[command(display_order = 4)]
    Back {},

    /// Show per-line AI/human attribution for a file
    #[command(
        display_order = 5,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo blame src/main.rs          Show AI attribution at HEAD\n  \
                       oobo blame src/main.rs abc123   At a specific commit\n  \
                       oobo blame src/main.rs --json   JSON output"
    )]
    Blame {
        /// Pure `git blame` output (no AI column).
        #[arg(long = "no-ai")]
        no_ai: bool,
        /// Arguments forwarded to `git blame` (plus AI overlay).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Search sessions and anchors (this project by default; --global for all)
    #[command(
        display_order = 6,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo search \"auth middleware\"      This project (inside a repo)\n  \
                       oobo search foo --global            Across all projects\n  \
                       oobo search foo --since 7d          Last 7 days\n  \
                       oobo search foo --project oobo-cli  Explicit project scope\n  \
                       oobo search foo --tool cursor       Scope to a tool\n  \
                       oobo search foo --agent             Compact output"
    )]
    Search {
        /// Free-text query (quote multi-word queries)
        query: Vec<String>,
        /// Search across all projects (default is the current project when in a repo)
        #[arg(long, conflicts_with = "project")]
        global: bool,
        /// Local only (default when no API key)
        #[arg(long, conflicts_with_all = ["remote", "both"])]
        local: bool,
        /// Remote server only (requires API key)
        #[arg(long, conflicts_with_all = ["local", "both"])]
        remote: bool,
        /// Local + remote merged (default when API key configured)
        #[arg(long, conflicts_with_all = ["local", "remote"])]
        both: bool,
        /// Time window (e.g. 7d, 24h, 30m, or ISO timestamp)
        #[arg(long)]
        since: Option<String>,
        /// Scope search to a specific project (by name)
        #[arg(long)]
        project: Option<String>,
        /// Scope hits to a single tool (claude, cursor, gemini...)
        #[arg(long)]
        tool: Option<String>,
        /// Max results (default 20)
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },

    /// Declarative KV config (no OAuth, no login flow)
    #[command(
        display_order = 6,
        after_help = "\x1b[1mGrammar:\x1b[0m  oobo settings [scope] [verb] <key> [value]\n\n\
                       \x1b[1mExamples:\x1b[0m\n  \
                       oobo settings                       Show all effective settings\n  \
                       oobo settings default               Show defaults only\n  \
                       oobo settings project               Show project overrides\n  \
                       oobo settings key                   Show the default 'key' value\n  \
                       oobo settings set key sk_abc        Set the default API key\n  \
                       oobo settings project set remote <url>  Per-project override\n  \
                       oobo settings unset key             Remove the default API key\n  \
                       oobo settings project unset remote  Drop the project override"
    )]
    Settings {
        /// Positional args: [scope] [verb] <key> [value]
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Start tracking the current project (idempotent)
    #[command(
        display_order = 7,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo enable           Turn on tracking for this repo\n  \
                       oobo enable --json    Machine-readable confirmation"
    )]
    Enable {},

    /// Stop tracking the current project (keeps existing anchors)
    #[command(
        display_order = 8,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo disable          Turn off tracking for this repo\n  \
                       oobo disable --agent  Minimal confirmation"
    )]
    Disable {},

    /// Onboarding + repair wizard
    #[command(
        display_order = 10,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo setup                        Interactive wizard\n  \
                       oobo setup --non-interactive      CI-safe: accept defaults\n  \
                       oobo setup --reindex              Force full reindex\n  \
                       oobo setup --repair               Re-install hooks + verify\n  \
                       oobo setup --repair --reindex     Composable"
    )]
    Setup {
        /// Accept defaults non-interactively (CI-safe)
        #[arg(long)]
        non_interactive: bool,
        /// Force a full reindex
        #[arg(long)]
        reindex: bool,
        /// Re-install hooks, re-detect tools, rebuild orphan branch if needed
        #[arg(long)]
        repair: bool,
    },

    /// Built-in documentation (oobo help <topic>)
    #[command(display_order = 15)]
    Help {
        /// Topic to display (omit for list of topics)
        topic: Option<String>,
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
pub enum AnchorAction {
    /// Drill into one commit's anchor by SHA (prefix OK if unambiguous)
    #[command(
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo anchor show a1b2c3d               Drill into a commit\n  \
                       oobo anchor show a1b2c3d --agent       Compact output\n  \
                       oobo anchor show a1b2c3d --json        Full JSON output"
    )]
    Show {
        /// Commit SHA (full or unambiguous prefix).
        sha: String,
    },

    /// Show per-line AI/human attribution (alias for `oobo blame`)
    #[command(
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       oobo blame src/main.rs          Show AI attribution at HEAD\n  \
                       oobo blame src/main.rs abc123   At a specific commit\n  \
                       oobo blame src/main.rs --json   JSON output"
    )]
    Blame {
        /// Pure `git blame` output (no AI column).
        #[arg(long = "no-ai")]
        no_ai: bool,
        /// Arguments forwarded to `git blame` (plus AI overlay).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

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
    /// Post-merge hook handler
    PostMerge {
        /// Args passed by git (ignored)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        _args: Vec<String>,
    },
    /// Post-rewrite hook handler
    PostRewrite {
        /// Rewrite command passed by git, e.g. amend or rebase
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        _args: Vec<String>,
    },
}

/// Re-parse the CLI with a synthetic argv and dispatch. Used by the legacy
/// hint system to rewrite e.g. `oobo scan` → `oobo setup --reindex`.
fn dispatch_with_argv(cfg: &Config, argv: Vec<String>) -> std::pin::Pin<Box<dyn std::future::Future<Output = CmdResult> + '_>> {
    Box::pin(async move {
        let cli = Cli::try_parse_from(argv).map_err(|e| format!("dispatch rewrite: {e}"))?;
        let mode = resolve_output_mode(cli.json, cli.agent, cli.interactive);
        dispatch_parsed(cfg, cli, mode).await
    })
}

/// Agent-env-var names. Any of these being set & non-empty implies agent mode.
const AGENT_ENV_VARS: &[&str] = &[
    "CURSOR_AGENT",
    "CLAUDECODE",
    "AIDER",
    "CONTINUE_SESSION",
    "CONTINUE_IDE",
    "AICOMMITS",
];

fn agent_env_active() -> bool {
    AGENT_ENV_VARS
        .iter()
        .any(|k| std::env::var(k).map(|v| !v.is_empty()).unwrap_or(false))
}

fn stdout_is_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// Resolve the effective output mode from explicit flags + environment.
///
/// Precedence (highest first):
/// 1. `--json` → Json
/// 2. `--agent` → Agent
/// 3. `--interactive` → Tui (force)
/// 4. Auto-detect: non-TTY stdout OR any agent env var → Agent
/// 5. Default → Tui (pretty)
pub fn resolve_output_mode(json: bool, agent: bool, interactive: bool) -> OutputMode {
    if json {
        return OutputMode::Json;
    }
    if agent {
        return OutputMode::Agent;
    }
    if interactive {
        return OutputMode::Tui;
    }
    if !stdout_is_tty() || agent_env_active() {
        return OutputMode::Agent;
    }
    OutputMode::Tui
}

/// Determine what to do and dispatch.
pub async fn route(cfg: &Config) -> CmdResult {
    let raw_args: Vec<String> = std::env::args().collect();

    if is_root_version_request(&raw_args) && raw_args.iter().any(|a| a == "--json") {
        if raw_args.iter().any(|a| a == "--agent") {
            eprintln!("error: the argument '--agent' cannot be used with '--json'");
            return Ok(2);
        }
        print_version_json();
        return Ok(0);
    }

    // Legacy 0.1.x command hints.
    if let Some(verb) = raw_args.get(1) {
        if let Some(hint) = crate::commands::legacy::lookup(verb) {
            if let Some(code) = crate::commands::legacy::handle(hint) { return Ok(code) }
            if let Some(mapped) = hint.mapped {
                let mut new_argv: Vec<String> = vec![raw_args[0].clone()];
                for m in mapped {
                    new_argv.push((*m).to_string());
                }
                return dispatch_with_argv(cfg, new_argv).await;
            }
            return Ok(2);
        }
    }

    let cli = Cli::parse();
    let mode = resolve_output_mode(cli.json, cli.agent, cli.interactive);

    dispatch_parsed(cfg, cli, mode).await
}

fn is_root_version_request(args: &[String]) -> bool {
    let mut saw_version = false;
    for arg in args.iter().skip(1) {
        match arg.as_str() {
            "--version" | "-V" => saw_version = true,
            "--json" | "--agent" | "--interactive" => {}
            _ => return false,
        }
    }
    saw_version
}

fn print_version_json() {
    let value = serde_json::json!({
        "name": "oobo",
        "version": env!("CARGO_PKG_VERSION"),
        "commit": option_env!("OOBO_BUILD_COMMIT").unwrap_or("unknown"),
        "built_at": option_env!("OOBO_BUILT_AT").unwrap_or("unknown"),
    });
    crate::utils::print_json(&value);
}

/// Extract the project root from a hook payload's `workspace_roots` field
/// without fully deserializing the payload. Returns `None` if the field is
/// missing or doesn't resolve to a git repo.
fn payload_project_root(payload: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(payload).ok()?;
    let root = parsed
        .get("workspace_roots")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())?;
    let resolved = git::proxy::project_root_from(root);
    if resolved.is_empty() {
        None
    } else {
        Some(resolved)
    }
}

/// Dispatch a parsed `Cli`. Extracted so legacy-hint rewrites can re-enter
/// the same code path after swapping argv.
async fn dispatch_parsed(cfg: &Config, cli: Cli, mode: OutputMode) -> CmdResult {
    let result = match cli.command {
        Some(Command::Anchors {}) => {
            run_anchors_feed(cfg, &cli, mode)
        }
        Some(Command::Anchor { action }) => {
            match action {
                AnchorAction::Show { sha } => {
                    let code = crate::commands::anchors::run_show(cfg, &sha, mode)?;
                    Ok(code)
                }
                AnchorAction::Blame { no_ai, args } => {
                    dispatch_blame(cfg, no_ai, args, mode)
                }
            }
        }
        Some(Command::Goto { target, no_stash }) => {
            let code = crate::commands::goto::run(cfg, &target, no_stash, mode)?;
            Ok(code)
        }
        Some(Command::Back {}) => {
            let code = crate::commands::goto::run_back(cfg, mode)?;
            Ok(code)
        }
        Some(Command::Blame { no_ai, args }) => {
            dispatch_blame(cfg, no_ai, args, mode)
        }
        Some(Command::Search {
            query,
            global,
            local,
            remote,
            both,
            since,
            project,
            tool,
            limit,
        }) => {
            let source = if local {
                Some(crate::commands::search::Source::Local)
            } else if remote {
                Some(crate::commands::search::Source::Remote)
            } else if both {
                Some(crate::commands::search::Source::Both)
            } else {
                None
            };

            let scope = if let Some(name) = project {
                crate::commands::search::Scope::Project(name)
            } else if global {
                crate::commands::search::Scope::Global
            } else {
                match crate::git::proxy::project_root(cfg) {
                    Some(root) => crate::commands::search::Scope::CurrentRepo(root),
                    None => crate::commands::search::Scope::Global,
                }
            };

            let q = query.join(" ");
            if mode == OutputMode::Tui {
                return crate::tui::app::run_search(cfg, &q);
            }
            let opts = crate::commands::search::Options {
                source,
                since,
                scope,
                tool,
                limit,
            };
            let code = crate::commands::search::run(cfg, &q, &opts, mode).await?;
            Ok(code)
        }
        Some(Command::Settings { args }) => {
            let code = crate::commands::settings::run(cfg, &args, mode)?;
            Ok(code)
        }
        Some(Command::Enable {}) => {
            let code = crate::commands::toggle::enable(cfg, mode)?;
            Ok(code)
        }
        Some(Command::Disable {}) => {
            let code = crate::commands::toggle::disable(cfg, mode)?;
            Ok(code)
        }
        Some(Command::Setup {
            non_interactive,
            reindex,
            repair,
        }) => {
            let opts = crate::setup::SetupOptions {
                non_interactive,
                reindex,
                repair,
                mode,
            };
            let code = crate::setup::run_setup_with(&opts)?;
            Ok(code)
        }
        Some(Command::Help { topic }) => {
            let code = crate::help::run(topic.as_deref(), mode);
            Ok(code)
        }
        Some(Command::Update { check, post_update }) => {
            if post_update {
                crate::commands::update::run_post_update()?;
            } else {
                crate::commands::update::run(check).await?;
            }
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

                    let project_root = payload_project_root(&payload)
                        .or_else(|| git::proxy::project_root(cfg));
                    if project_root
                        .as_deref().is_none_or(|x| !crate::project_config::is_enabled(x))
                    {
                        return Ok(0);
                    }

                    tracing::debug!(event = %event, tool = ?tool, payload_len = payload.len(), "hook event received");
                    crate::hooks::handle_event(&event, &payload, tool.as_deref())
                        .map_err(|e| e.to_string())?;
                }
                HookAction::PostCommit { .. } => {
                    if let Some(root) = git::proxy::project_root(cfg) {
                        if !crate::project_config::is_enabled(&root) {
                            return Ok(0);
                        }
                        if std::env::var("OOBO_INTERCEPTED").is_err() {
                            if let Err(e) = crate::git::interceptor::on_write_op(cfg, &["commit"])
                            {
                                eprintln!("oobo: warning: {e}");
                            }
                        }
                        crate::hooks::state::cleanup_stale(&root, 86400);
                    }
                }
                HookAction::PrePush { .. } => {
                    if let Some(root) = git::proxy::project_root(cfg) {
                        if !crate::project_config::is_enabled(&root) {
                            return Ok(0);
                        }
                        if crate::git::orphan::branch_exists(&root) {
                            if let Err(e) = crate::git::orphan::push(&root) {
                                eprintln!("oobo: warning: could not push anchors: {e}");
                            }
                        }
                        crate::git::orphan::retry_pending_pushes(&root);
                    }
                }
                HookAction::PostMerge { .. } => {
                    if let Some(root) = git::proxy::project_root(cfg) {
                        if !crate::project_config::is_enabled(&root) {
                            return Ok(0);
                        }
                        crate::commands::sync::auto_hydrate(&root);
                    }
                }
                HookAction::PostRewrite { .. } => {
                    let mut payload = String::new();
                    if let Err(e) =
                        std::io::Read::read_to_string(&mut std::io::stdin(), &mut payload)
                    {
                        eprintln!("oobo: warning: could not read post-rewrite payload: {e}");
                    }
                    if let Some(root) = git::proxy::project_root(cfg) {
                        if !crate::project_config::is_enabled(&root) {
                            return Ok(0);
                        }
                        let pairs = crate::git::orphan::parse_rewrite_pairs(&payload);
                        if let Err(e) =
                            crate::git::orphan::rekey_anchors_from_rewrite_pairs(&root, &pairs)
                        {
                            eprintln!("oobo: warning: could not update rewritten anchors: {e}");
                        }
                    }
                }
            }
            Ok(0)
        }
        None => {
            // Bare `oobo` = same as `oobo anchors` (the memory feed).
            run_anchors_feed(cfg, &cli, mode)
        }
    };

    result
}

/// Shared logic for both `oobo` (bare) and `oobo anchors`.
fn run_anchors_feed(cfg: &Config, cli: &Cli, mode: OutputMode) -> CmdResult {
    if mode == OutputMode::Tui {
        crate::commands::bare::run(cfg, mode)
    } else {
        let opts = crate::commands::anchors::Options {
            limit: cli.limit,
            since: cli.since.clone(),
            tool: cli.tool.clone(),
        };
        let code = crate::commands::anchors::run_list(cfg, &opts, mode)?;
        Ok(code)
    }
}

fn dispatch_blame(cfg: &Config, no_ai: bool, mut args: Vec<String>, mode: OutputMode) -> CmdResult {
    let mut mode = mode;
    let mut local_json = false;
    let mut local_agent = false;
    args.retain(|a| match a.as_str() {
        "--json" => {
            local_json = true;
            false
        }
        "--agent" => {
            local_agent = true;
            false
        }
        "--interactive" => false,
        _ => true,
    });
    if local_json && local_agent {
        eprintln!("error: --agent and --json are mutually exclusive");
        return Ok(2);
    }
    if local_json {
        mode = OutputMode::Json;
    } else if local_agent {
        mode = OutputMode::Agent;
    }
    let code = crate::commands::blame::run(cfg, no_ai, &args, mode)?;
    Ok(code)
}
