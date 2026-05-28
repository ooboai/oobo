//! Built-in help system  --  `oobo help <topic>`
//!
//! Rich prose-based help compiled into the binary. Always available, always
//! current, works offline.

use crate::cli::OutputMode;

pub const TOPICS: &[(&str, &str)] = &[
    ("anchors", "What are anchors and how do they work"),
    ("search", "Semantic code search (hybrid BM25 + vector)"),
    (
        "recall",
        "Session/anchor search  --  syntax, filters, cloud vs local",
    ),
    ("blame", "Reading the AI attribution overlay"),
    ("hooks", "How git and agent hooks capture sessions"),
    ("config", "All settings explained"),
    ("keyboard", "TUI keybindings reference"),
];

pub fn run(topic: Option<&str>, mode: OutputMode) -> i32 {
    let Some(t) = topic else {
        list_topics(mode);
        return 0;
    };
    if let Some(content) = lookup(t) {
        emit(t, content, mode);
        0
    } else {
        eprintln!("error: unknown topic '{t}'");
        eprintln!();
        eprintln!("available topics:");
        for (name, desc) in TOPICS {
            eprintln!("  {name:<12} {desc}");
        }
        2
    }
}

fn list_topics(mode: OutputMode) {
    match mode {
        OutputMode::Json => {
            let arr: Vec<serde_json::Value> = TOPICS
                .iter()
                .map(|(name, desc)| serde_json::json!({"topic": name, "description": desc}))
                .collect();
            crate::utils::print_json(&serde_json::json!({"topics": arr}));
        }
        OutputMode::Agent => {
            for (name, desc) in TOPICS {
                println!("{name} {desc}");
            }
        }
        OutputMode::Tui => {
            println!("\x1b[1moobo help\x1b[0m  --  built-in documentation\n");
            println!("  Usage: oobo help <topic>\n");
            for (name, desc) in TOPICS {
                println!("  \x1b[1m{name:<12}\x1b[0m {desc}");
            }
            println!();
        }
    }
}

fn emit(topic: &str, content: &str, mode: OutputMode) {
    match mode {
        OutputMode::Json => {
            let json = serde_json::json!({"topic": topic, "content": content});
            crate::utils::print_json(&json);
        }
        OutputMode::Agent => {
            println!("{content}");
        }
        OutputMode::Tui => {
            println!("\x1b[1moobo help {topic}\x1b[0m\n");
            println!("{content}");
        }
    }
}

fn lookup(topic: &str) -> Option<&'static str> {
    match topic {
        "anchors" => Some(HELP_ANCHORS),
        "search" => Some(HELP_CODE_SEARCH),
        "recall" => Some(HELP_SEARCH),
        "blame" => Some(HELP_BLAME),
        "hooks" => Some(HELP_HOOKS),
        "config" => Some(HELP_CONFIG),
        "keyboard" => Some(crate::tui::status::KEYBOARD_GRAMMAR),
        _ => None,
    }
}

const HELP_ANCHORS: &str = "\
Anchors are the core unit of oobo memory. Every git commit that was made
while an AI coding tool was active gets an anchor  --  a metadata record that
links the commit to the AI session(s) that contributed to it.

An anchor captures:
  - Which AI tool(s) were active (Cursor, Claude Code, Codex, etc.)
  - Token usage (input, output, cache)
  - AI contribution percentage (line-level attribution)
  - The model used (claude-opus-4, gpt-4o, etc.)
  - Optionally, the full transcript (when transparency is on)

Anchors are stored on a git orphan branch (oobo/anchors/v1) that syncs
with your remote. They travel with your code.

Working memory (shadows) are live session snapshots that appear before
you commit. They become anchors when the commit happens.

Commands:
  oobo                    View the memory feed (TUI)
  oobo anchors --agent    List anchors (plain text)
  oobo anchor show <sha>  Inspect a specific anchor
";

const HELP_SEARCH: &str = "\
Recall finds past sessions and anchors by matching your query against
commit messages, intents, and session content.

Usage:
  oobo recall \"auth middleware\"     Search this project
  oobo recall \"query\" --json        Structured output
  oobo recall \"query\" --since 7d    Last 7 days only
  oobo recall \"query\" --tool cursor Scope to a tool

Search sources:
  - Local: reads from the oobo/anchors/v1 branch in this repo
  - Cloud: queries the oobo.ai API (requires API key)
  - Default: local + cloud when a key is configured

Configure cloud search:
  oobo settings set key <your-api-key>
";

const HELP_CODE_SEARCH: &str = "\
Semantic code search finds relevant code by meaning, not just keywords.
Powered by sonar (hybrid BM25 + vector search).

Usage:
  oobo search \"auth middleware\"       Search this repo
  oobo search \"parse config\" -k 10   Top 10 results
  oobo search \"query\" --mode bm25    Keyword only (fastest)
  oobo search \"query\" --mode semantic Vector only
  oobo search \"query\" --content docs  Search docs instead of code
  oobo search \"query\" --content all   Code + docs + config

The index is built automatically on first search and cached.
Re-indexing happens when files change.
";

const HELP_BLAME: &str = "\
oobo blame traces every line back to the commit that introduced it, then
loads that commit's anchor to resolve AI vs human attribution. This gives
historically accurate blame across the full file history.

Usage:
  oobo blame src/main.rs              Color-coded blame at HEAD
  oobo blame src/main.rs abc123       At a specific commit
  oobo blame src/main.rs --agent      Compact text columns
  oobo blame src/main.rs --json       Rich per-line JSON (for plugins)

Color coding (pretty mode):
  magenta  = AI-generated (tool name shown, e.g. cursor, claude)
  yellow   = mixed (both AI and human contributed)
  green    = human-written
  dim gray = unknown (pre-oobo or no anchor)

JSON output includes per line: origin_sha, ai, agent, session_ids,
tokens, committed_at, and commit message  --  designed for IDE plugins.

Every git blame flag is forwarded. Machine-output formats (--porcelain,
--line-porcelain, --incremental) bypass the AI column automatically.
";

const HELP_HOOKS: &str = "\
oobo uses two types of hooks to capture AI sessions:

Git hooks (installed in .git/hooks/):
  post-commit   Captures anchor metadata after each commit
  pre-push      Syncs anchors to remote before push
  post-merge    Re-indexes after merge
  post-rewrite  Updates anchors after rebase/amend

Agent hooks (installed in tool config, e.g. ~/.cursor/hooks.json):
  stop              Captures session end (turn boundary)
  preToolUse        Snapshots file before AI edit
  postToolUse       Records file after AI edit
  subagentStart     Tracks subagent spawning
  subagentStop      Records subagent completion

Install/repair hooks:
  oobo enable       Install hooks for this project
  oobo setup        Full repair wizard
";

const HELP_CONFIG: &str = "\
oobo settings are layered: defaults apply globally, project overrides
take precedence when inside a repo with .oobo/config.

Settings:
  key              API key for oobo.ai cloud features
  api_url          API endpoint URL (default: https://api.oobo.ai)
  remote           Git remote name for anchor branch sync (default: origin)
  transparency     Sync transcripts to anchors branch (on/off, default: on)
  tools.experimental  Enable experimental tool adapters (on/off)

Commands:
  oobo settings                       Show all effective settings
  oobo settings set key <value>       Set a default
  oobo settings unset key             Remove a setting
  oobo settings project set api_url X Per-project override

Files:
  ~/.oobo/config.toml    Global defaults
  .oobo/config           Per-project overrides (git-ignored cache dir)
";
