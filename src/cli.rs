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
    #[allow(dead_code)]
    pub fn is_structured(self) -> bool {
        matches!(self, OutputMode::Agent | OutputMode::Json)
    }
}

/// anchor — git with memory
#[derive(Parser, Debug)]
#[command(
    name = "anchor",
    version,
    about = "anchor — git with memory.",
    help_template = "\
{about}

USAGE:
    anchor [OPTIONS] [COMMAND]

VIEWS:
  anchors, a   See the memory
  blame        Per-line AI/human attribution
  search       Find any past session

ACTIONS:
  from        Load code/context from a turn or anchor
  enable       Start tracking this project
  disable      Stop tracking this project
  alias        Install/uninstall the git=anchor shell alias

WIZARD + CONFIG:
  setup        Onboard, repair, reindex, manage projects
  settings     Show / set / unset config values

LIFECYCLE:
  update       Self-update

GIT PASSTHROUGH:
  Any command not listed above is forwarded to git unchanged.
  Write operations (commit, push, merge) also capture AI context.

OPTIONS:
  --agent          Minimal plain-text output (token-efficient)
  --json           Full structured JSON output
  --interactive    Force TUI even when auto-detection would not
  -h, --help       Print help
  -V, --version    Print version

Run `anchor <command> --help` for per-command help.
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

    /// Raw args passed when invoked as a git alias (everything after `anchor`)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub git_args: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum AnchorsAction {
    /// Drill into one anchor by SHA (prefix OK if unambiguous).
    Show {
        /// Commit SHA (full or unambiguous prefix).
        sha: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Show enriched commit history with anchor metadata
    #[command(
        display_order = 1,
        alias = "a",
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       anchor anchors               Show recent commits with AI context\n  \
                       anchor a -n 20               Show last 20 (short alias)\n  \
                       anchor anchors --agent       Compact output\n  \
                       anchor anchors --json        Full JSON output"
    )]
    Anchors {
        /// Subcommand (e.g. `show <sha>`). Omit to list anchors.
        #[command(subcommand)]
        action: Option<AnchorsAction>,
        /// Max anchors to list (default 50)
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: usize,
        /// Only anchors at/after this point (e.g. 24h, 7d, ISO-8601).
        #[arg(long)]
        since: Option<String>,
        /// Filter by tool name (case-insensitive).
        #[arg(long)]
        tool: Option<String>,
        /// Filter/scope to a specific project (valid only OUTSIDE a repo).
        #[arg(long)]
        project: Option<String>,
    },

    /// Show per-line AI/human attribution for a file
    #[command(
        display_order = 2,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       anchor blame src/main.rs          Show AI attribution for file at HEAD\n  \
                       anchor blame src/main.rs abc123   Show attribution at a specific commit\n  \
                       anchor blame src/main.rs --json   JSON output"
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
        display_order = 3,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       anchor search \"auth middleware\"      This project (inside a repo)\n  \
                       anchor search foo --global            Across all projects\n  \
                       anchor search foo --since 7d          Last 7 days\n  \
                       anchor search foo --project oobo-cli  Explicit project scope\n  \
                       anchor search foo --tool cursor       Scope to a tool\n  \
                       anchor search foo --agent             Compact output"
    )]
    Search {
        /// Free-text query (quote multi-word queries)
        query: Vec<String>,
        /// Search across all projects (default is the current project when in a repo)
        #[arg(long, conflicts_with = "project")]
        global: bool,
        /// Local DB only (default when no API key)
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
        /// Explicit project to scope to (by name); implies cross-project search
        #[arg(long)]
        project: Option<String>,
        /// Scope hits to a single tool (claude, cursor, gemini...)
        #[arg(long)]
        tool: Option<String>,
        /// Max results (default 20)
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },

    /// Load code/context from a turn or anchor (preview by default)
    #[command(
        display_order = 5,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       anchor from turn t123abc          Preview a turn\n  \
                       anchor from turn t123abc --load   Load that turn into the worktree\n  \
                       anchor from anchor abc123 --load  Load an anchor commit"
    )]
    From {
        #[command(subcommand)]
        action: FromAction,
    },

    /// Declarative KV config (no OAuth, no login flow)
    #[command(
        display_order = 6,
        after_help = "\x1b[1mGrammar:\x1b[0m  anchor settings [scope] [verb] <key> [value]\n\n\
                       \x1b[1mExamples:\x1b[0m\n  \
                       anchor settings                       Show all effective settings\n  \
                       anchor settings default               Show defaults only\n  \
                       anchor settings project               Show project overrides\n  \
                       anchor settings key                   Show the default 'key' value\n  \
                       anchor settings set key sk_abc        Set the default API key\n  \
                       anchor settings project set remote <url>  Per-project override\n  \
                       anchor settings unset key             Remove the default API key\n  \
                       anchor settings project unset remote  Drop the project override"
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
                       anchor enable           Turn on tracking for this repo\n  \
                       anchor enable --json    Machine-readable confirmation"
    )]
    Enable {},

    /// Stop tracking the current project (keeps existing anchors)
    #[command(
        display_order = 8,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       anchor disable          Turn off tracking for this repo\n  \
                       anchor disable --agent  Minimal confirmation"
    )]
    Disable {},

    /// Onboarding + repair wizard (projects, hooks, keys, alias)
    #[command(
        display_order = 10,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       anchor setup                        Interactive wizard\n  \
                       anchor setup --non-interactive      CI-safe: accept defaults\n  \
                       anchor setup --reindex              Force full reindex\n  \
                       anchor setup --repair               Re-install hooks + verify\n  \
                       anchor setup --uninstall-alias      Remove the shell alias\n  \
                       anchor setup --repair --reindex     Composable"
    )]
    Setup {
        /// Accept defaults non-interactively (CI-safe)
        #[arg(long)]
        non_interactive: bool,
        /// Force a full reindex
        #[arg(long)]
        reindex: bool,
        /// Remove the git→anchor shell alias
        #[arg(long)]
        uninstall_alias: bool,
        /// Re-install hooks, re-detect tools, rebuild orphan branch if needed
        #[arg(long)]
        repair: bool,
    },

    /// Manage the git→anchor shell alias [install, uninstall]
    #[command(
        display_order = 11,
        after_help = "\x1b[1mExamples:\x1b[0m\n  \
                       anchor alias install     Alias git→anchor in your shell\n  \
                       anchor alias uninstall   Remove the alias"
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
    /// Add `alias git=anchor` to your shell RC file
    Install,
    /// Remove the git→anchor alias from your shell RC file
    Uninstall,
}

#[derive(Subcommand, Debug)]
pub enum FromAction {
    /// Preview or load a shadow-anchor snapshot
    Turn {
        /// Turn id (full or unambiguous prefix)
        turn_id: String,
        /// Actually load the turn into the worktree. Omit for preview.
        #[arg(long)]
        load: bool,
        /// Permit loading over a dirty worktree.
        #[arg(long)]
        force: bool,
    },
    /// Preview or load an anchor commit
    Anchor {
        /// Commit SHA (full or unambiguous prefix)
        sha: String,
        /// Actually load the anchor into the worktree. Omit for preview.
        #[arg(long)]
        load: bool,
        /// Permit loading over a dirty worktree.
        #[arg(long)]
        force: bool,
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

/// Reserved anchor verbs. Anything else at argv[1] is forwarded to `git` (passthrough).
const OOBO_SUBCOMMANDS: &[&str] = &[
    "anchors", "a", "from", "blame", "search", "settings", "enable", "disable", "setup", "alias",
    "update", "hooks",
];

/// Re-parse the CLI with a synthetic argv and dispatch. Used by the legacy
/// hint system to rewrite e.g. `anchor scan` → `anchor setup --reindex`.
fn dispatch_with_argv(cfg: Config, argv: Vec<String>) -> Result<i32, String> {
    let cli = Cli::try_parse_from(argv).map_err(|e| format!("dispatch rewrite: {e}"))?;
    let mode = resolve_output_mode(cli.json, cli.agent, cli.interactive);
    dispatch_parsed(cfg, cli, mode)
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

/// Commands that read data — safe to kick a background scan for.
fn is_view_command(cmd: &Option<Command>) -> bool {
    matches!(
        cmd,
        None | Some(Command::Anchors { .. })
            | Some(Command::Blame { .. })
            | Some(Command::Search { .. })
    )
}

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

    if is_root_version_request(&raw_args) && raw_args.iter().any(|a| a == "--json") {
        if raw_args.iter().any(|a| a == "--agent") {
            eprintln!("error: the argument '--agent' cannot be used with '--json'");
            return Ok(2);
        }
        print_version_json();
        return Ok(0);
    }

    // Legacy 0.1.x command hints. Fires BEFORE git passthrough so we can
    // intercept names that collide with git verbs only coincidentally.
    if let Some(verb) = raw_args.get(1) {
        if !OOBO_SUBCOMMANDS.contains(&verb.as_str()) {
            if let Some(hint) = crate::commands::legacy::lookup(verb) {
                match crate::commands::legacy::handle(hint) {
                    Some(code) => return Ok(code),
                    None => {
                        // Continue with mapped args.
                        if let Some(mapped) = hint.mapped {
                            let mut new_argv: Vec<String> = vec![raw_args[0].clone()];
                            for m in mapped {
                                new_argv.push((*m).to_string());
                            }
                            // Replace argv in-process so clap sees the rewrite.
                            return dispatch_with_argv(cfg, new_argv);
                        }
                        return Ok(2);
                    }
                }
            }
        }
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

    let mode = resolve_output_mode(cli.json, cli.agent, cli.interactive);

    dispatch_parsed(cfg, cli, mode)
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
        "name": "anchor",
        "version": env!("CARGO_PKG_VERSION"),
        "commit": option_env!("OOBO_BUILD_COMMIT").unwrap_or("unknown"),
        "built_at": option_env!("OOBO_BUILT_AT").unwrap_or("unknown"),
    });
    crate::utils::print_json(&value);
}

/// Dispatch a parsed `Cli`. Extracted so legacy-hint rewrites can re-enter
/// the same code path after swapping argv.
fn dispatch_parsed(cfg: Config, cli: Cli, mode: OutputMode) -> Result<i32, String> {
    // Fire-and-forget auto-index for view-style commands. Never blocks.
    if is_view_command(&cli.command) {
        crate::commands::auto::maybe_kick(&cfg);
    }

    let result = match cli.command {
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

            // Scope resolution:
            //   explicit --project NAME → that project
            //   --global                → all projects
            //   inside a repo (no flag) → current project
            //   outside a repo          → all projects
            let scope = if let Some(name) = project {
                crate::commands::search::Scope::Project(name)
            } else if global {
                crate::commands::search::Scope::Global
            } else {
                match crate::git::proxy::project_root(&cfg) {
                    Some(root) => crate::commands::search::Scope::CurrentRepo(root),
                    None => crate::commands::search::Scope::Global,
                }
            };

            let q = query.join(" ");
            let opts = crate::commands::search::Options {
                source,
                since,
                scope,
                tool,
                limit,
            };
            let code = crate::commands::search::run(&cfg, &q, opts, mode)?;
            Ok(code)
        }
        Some(Command::Settings { args }) => {
            let code = crate::commands::settings::run(&cfg, &args, mode)?;
            Ok(code)
        }
        Some(Command::From { action }) => {
            let code = match action {
                FromAction::Turn {
                    turn_id,
                    load,
                    force,
                } => crate::commands::from::run_turn(&cfg, &turn_id, load, force, mode)?,
                FromAction::Anchor { sha, load, force } => {
                    crate::commands::from::run_anchor(&cfg, &sha, load, force, mode)?
                }
            };
            Ok(code)
        }
        Some(Command::Enable {}) => {
            let code = crate::commands::toggle::enable(&cfg, mode)?;
            Ok(code)
        }
        Some(Command::Disable {}) => {
            let code = crate::commands::toggle::disable(&cfg, mode)?;
            Ok(code)
        }
        Some(Command::Setup {
            non_interactive,
            reindex,
            uninstall_alias,
            repair,
        }) => {
            let opts = crate::setup::SetupOptions {
                non_interactive,
                reindex,
                uninstall_alias,
                repair,
                mode,
            };
            let code = crate::setup::run_setup_with(opts).map_err(|e| e.to_string())?;
            Ok(code)
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
        Some(Command::Anchors {
            action,
            limit,
            since,
            tool,
            project,
        }) => {
            let opts = crate::commands::anchors::Options {
                limit,
                since,
                tool,
                project,
            };
            match action {
                Some(AnchorsAction::Show { sha }) => {
                    let code = crate::commands::anchors::run_show(&cfg, &sha, mode)?;
                    Ok(code)
                }
                None => {
                    let code = crate::commands::anchors::run_list(&cfg, opts, mode)?;
                    Ok(code)
                }
            }
        }
        Some(Command::Blame { no_ai, mut args }) => {
            // `trailing_var_arg` slurps global flags when they appear
            // after the first positional. Recover them here.
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
            let code = crate::commands::blame::run(&cfg, no_ai, &args, mode)?;
            Ok(code)
        }
        Some(Command::Hooks { action }) => {
            match action {
                HookAction::Agent { event, tool } => {
                    if git::proxy::project_root(&cfg)
                        .as_deref()
                        .map(crate::project_config::is_enabled)
                        != Some(true)
                    {
                        return Ok(0);
                    }
                    let mut payload = String::new();
                    if let Err(e) =
                        std::io::Read::read_to_string(&mut std::io::stdin(), &mut payload)
                    {
                        eprintln!("anchor: warning: could not read agent payload from stdin: {e}");
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
                        if !crate::project_config::is_enabled(&root) {
                            return Ok(0);
                        }
                        if std::env::var("OOBO_INTERCEPTED").is_err() {
                            if let Err(e) = crate::git::interceptor::on_write_op(&cfg, &["commit"])
                            {
                                eprintln!("anchor: warning: {e}");
                            }
                        }
                        crate::hooks::state::cleanup_stale(&root, 86400);
                    }
                }
                HookAction::PrePush { .. } => {
                    if let Some(root) = git::proxy::project_root(&cfg) {
                        if !crate::project_config::is_enabled(&root) {
                            return Ok(0);
                        }
                        if crate::git::orphan::branch_exists(&root) {
                            if let Err(e) = crate::git::orphan::push(&root) {
                                eprintln!("anchor: warning: could not push anchors: {e}");
                            }
                        }
                        crate::git::orphan::retry_pending_pushes(&root);
                    }
                }
                HookAction::PostMerge { .. } => {
                    if let Some(root) = git::proxy::project_root(&cfg) {
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
                        eprintln!("anchor: warning: could not read post-rewrite payload: {e}");
                    }
                    if let Some(root) = git::proxy::project_root(&cfg) {
                        if !crate::project_config::is_enabled(&root) {
                            return Ok(0);
                        }
                        let pairs = crate::git::orphan::parse_rewrite_pairs(&payload);
                        if let Err(e) =
                            crate::git::orphan::rekey_anchors_from_rewrite_pairs(&root, &pairs)
                        {
                            eprintln!("anchor: warning: could not update rewritten anchors: {e}");
                        }
                    }
                }
            }
            Ok(0)
        }
        None => {
            // Truly bare `anchor` (no trailing tokens) → four-quadrant view.
            // `anchor <non-reserved-verb>` (e.g. `anchor commit`, `anchor status`)
            // lands here too because clap parks unknown verbs in git_args;
            // those should still be forwarded to git (passthrough).
            if cli.git_args.is_empty() {
                let code = crate::commands::bare::run(&cfg, mode)?;
                Ok(code)
            } else {
                let git_args: Vec<&str> = cli.git_args.iter().map(|s| s.as_str()).collect();
                git::proxy::run_and_intercept(&cfg, &git_args)
            }
        }
    };

    result
}
