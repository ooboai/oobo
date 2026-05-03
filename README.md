<div align="center">

<img src="assets/icon.png" alt="oobo" width="120" />

# oobo

### Git for agents (and humans).

A transparent git decorator that enriches every commit with AI context:<br/>sessions, tokens, and code attribution — across 15 AI coding tools.<br/>No workflow changes. No plugins. No cloud required.

[![License](https://img.shields.io/badge/license-Apache%202.0%20%2F%20MIT-blue)](LICENSE-APACHE)
[![CI](https://img.shields.io/github/actions/workflow/status/ooboai/oobo/ci.yml?label=CI)](https://github.com/ooboai/oobo/actions/workflows/ci.yml)
[![GitHub Release](https://img.shields.io/github/v/release/ooboai/oobo?color=green)](https://github.com/ooboai/oobo/releases)
[![Issues](https://img.shields.io/github/issues/ooboai/oobo)](https://github.com/ooboai/oobo/issues)
[![Stars](https://img.shields.io/github/stars/ooboai/oobo?style=social)](https://github.com/ooboai/oobo)

[Documentation](https://docs.oobo.ai) · [Report Bug](https://github.com/ooboai/oobo/issues/new?labels=bug) · [Request Feature](https://github.com/ooboai/oobo/issues/new?labels=enhancement) · [Contributing](CONTRIBUTING.md)

</div>

---

## Installation

```bash
curl -fsSL https://oobo.ai/install.sh | bash
```

**Platforms:** macOS (Apple Silicon, Intel) · Linux (x86_64, ARM64, glibc, musl)

Or grab a binary from [Releases](https://github.com/ooboai/oobo/releases).

---

## Features

- **Transparent Git Hooks** — Works alongside your normal `git` workflow. Post-commit hooks automatically capture AI context without changing how you work.
- **AI Session Tracking** — Automatically discovers and links AI chat sessions to your commits — which agent wrote what, how many tokens it took, and which conversation produced each change.
- **15 Tools Supported** — Cursor, Claude Code, Gemini CLI, OpenCode, Codex, Aider, GitHub Copilot, Windsurf, Zed, Trae, Amp, Continue, Factory Droid, Junie, and Kiro.
- **Code Attribution** — Know exactly which lines were AI-generated vs human-written, per commit.
- **Agent-Native** — Three output modes (pretty / `--agent` token-efficient plain text / `--json` structured). `--agent` auto-activates when stdout isn't a TTY or inside a coding agent.
- **Local-First, Private by Default** — Anchors live in your git repo (orphan branch), with a lightweight local cache in `~/.oobo/`. Nothing leaves your machine unless you opt in. No telemetry. Secrets are redacted before sharing.
- **Anchor System** — Extends git commits with structured AI metadata that travels with the repo via a git orphan branch. No external dependencies.

---

## Quick Start

```bash
# 1. Install — setup runs automatically, detects your tools, configures everything
curl -fsSL https://oobo.ai/install.sh | bash

# 2. Use git normally — hooks capture AI context on every commit
git commit -m "fix auth middleware"
git push origin main

# 3. See what happened
oobo                            # in-repo: scrollable feed of your anchors
oobo anchor show <sha>          # drill into one anchor (sessions, tokens, attribution)
oobo goto <turn-or-sha>         # time-travel to a turn or commit (auto-stashes)
oobo back                       # return to where you were
oobo blame src/main.rs          # git blame + per-line AI attribution
oobo search "auth bug"          # search sessions + anchors
```

**Optionally**, add an [oobo.ai](https://oobo.ai) API key for **remote search** (and other authenticated API calls). Anchor metadata still syncs through Git by default.

```bash
oobo settings set key <your_key>
```

---

## How It Works

```
You run:  git commit -m "fix auth middleware"

  1. Post-commit hook fires
  2. Hook calls `oobo hooks post-commit`
  3. Oobo reads AI sessions from local tool storage
  4. Builds anchor: commit + sessions + tokens + attribution
  5. Writes anchor to git orphan branch
```

Git operations work exactly as normal — oobo captures context via hooks, not by wrapping git.

### The anchor

An **anchor** is oobo's core primitive — it extends a git commit with AI context:

```
Git:     commit = diff(files)
Anchor:  anchor = commit + sessions + tokens + attribution
```

Each anchor records which AI sessions contributed, token counts, code attribution (AI vs human lines), model used, and session duration. Anchors live on a git orphan branch (`oobo/anchors/v1`) that travels with the repo — no external dependencies.

---

## Supported Tools

| Tool                | Sessions | Transcripts | Token Stats | Agent Hooks |
| ------------------- | -------- | ----------- | ----------- | ----------- |
| Cursor              | ✓        | ✓           | ✓           | ✓           |
| Claude Code         | ✓        | ✓           | ✓           | ✓           |
| Gemini CLI          | ✓        | ✓           | ✓           | ✓           |
| OpenCode            | ✓        | ✓           | ✓           | ✓           |
| Codex CLI           | ✓        | ✓           | ✓           | —           |
| Aider               | ✓        | ✓           | ✓           | —           |
| GitHub Copilot Chat | ✓        | ✓           | ✓           | —           |
| Windsurf            | ✓        | ✓           | partial     | —           |
| Zed                 | ✓        | ✓           | ✓           | —           |
| Trae                | ✓        | ✓           | partial     | —           |
| Amp                 | ✓        | ✓           | —           | —           |
| Continue            | ✓        | ✓           | —           | ✓           |
| Factory Droid       | ✓        | ✓           | —           | ✓           |
| Junie               | ✓        | ✓           | —           | —           |
| Kiro                | ✓        | ✓           | —           | ✓           |

All tools are read-only — oobo never writes to AI tool data directories.

---

## For AI Agents

Oobo is built for agents. Agents commit code constantly, across tools, often in parallel. Without oobo, there is no record of which agent wrote what, how many tokens it took, or which conversation produced a given function.

### Agent install

```bash
curl -fsSL https://oobo.ai/install.sh | bash -s -- --agent
# → {"status":"ok","version":"...","binary":"...","platform":"..."}
```

The `--agent` flag suppresses colors and interactive prompts and returns a single JSON line.

### Output modes

Every command has three mutually exclusive output modes:

- **Pretty (default)** — rich TTY output with color, alignment, and interactive TUIs where available.
- **`--agent`** — token-efficient plain text, similar in spirit to `git log --oneline`. Auto-activates when stdout is not a TTY or one of `CURSOR_AGENT`, `CLAUDECODE`, `AIDER`, `CONTINUE_SESSION`, `CONTINUE_IDE`, `AICOMMITS` is set.
- **`--json`** — full-fidelity structured JSON for scripts and programmatic use (`jq`-parseable).

```bash
oobo --agent                         # token-efficient commit feed
oobo --json                          # flat JSON array of anchors
oobo blame src/main.rs --json        # per-line AI attribution as JSON
oobo search "auth" --agent           # compact search results
```

### Skill file

Oobo installs a skill file at `~/.oobo/skills/oobo/SKILL.md` during `oobo setup`, with symlinks in `~/.agents/skills/oobo/`, `~/.claude/skills/oobo/`, `~/.codex/skills/oobo/`, `~/.cursor/skills/oobo/`, and `~/.gemini/skills/oobo/`. AI coding tools discover the skill automatically and know how to install and use oobo.

### Agent lifecycle hooks

For tools that support it (Cursor, Claude Code, Gemini CLI, OpenCode, Kiro, Continue, Factory Droid), oobo installs hooks that track agent activity in real time: session start/end, tool calls, subagent spawns, thinking events, and context compaction. This enables precise session linking and rich telemetry attached to every commit anchor.

---

## Commands

### Bare `oobo`

```bash
oobo                             # in-repo + TTY: scrollable anchor-feed TUI
                                 # in-repo + --agent/--json: list anchors
                                 # outside a repo: first-run hint or short status
oobo -n 20 --since 7d           # filtered
oobo --tool cursor               # per-tool
oobo --project myapp             # outside a repo, aggregate one project
```

### Show — drill into a commit

```bash
oobo anchor show <sha>           # drill-down: sessions, tokens, attribution
oobo anchor show <sha> --json    # structured JSON for scripts
```

### Goto / Back — time travel

```bash
oobo goto <turn-id-or-sha>              # travel to a turn or commit
oobo goto <id> --no-stash               # fail if worktree is dirty
oobo back                               # return to where you were
```

`goto` auto-stashes dirty changes, loads the target tree, and records a return point. Multiple `goto` calls stack — each `back` pops one level, like a browser back button.

### Blame — git blame + AI attribution

```bash
oobo blame src/main.rs                       # git blame with an extra AI column
oobo blame src/main.rs @abc123               # at a specific commit
oobo blame --no-ai src/main.rs               # byte-identical to `git blame`
oobo blame src/main.rs --json                # per-line AI attribution as JSON
```

Every `git blame` flag (`-L`, `-w`, `--porcelain`, etc.) is forwarded; machine-output formats (`--porcelain`, `--line-porcelain`, `--incremental`) bypass the AI column automatically.

### Search — find sessions + anchors

```bash
oobo search "auth bug"                       # full-text search
oobo search "auth" --since 7d --tool claude --project myapp
oobo search "auth" --json                    # structured results
```

### Settings — declarative per-scope config

```bash
oobo settings                                # list default-scope keys
oobo settings key                            # get the API key (default scope)
oobo settings set key sk_...                 # set API key (remote search)
oobo settings set remote https://oobo.mycompany.com
oobo settings project set remote oobo        # push anchor branch to git remote "oobo"
oobo settings unset transparency             # remove a key
```

Scopes: `default` (implicit) or `project`. Verbs: `get` (default), `set`, `unset`.

### Per-project toggles

```bash
oobo enable                                  # start tracking this repo
oobo disable                                 # stop tracking this repo
```

### Setup & maintenance

```bash
oobo setup                                   # interactive wizard: install hooks, discover tools, seed config
oobo setup --non-interactive                 # for scripts + first-run agents
oobo setup --reindex                         # forced full rescan
oobo setup --repair                          # fix broken symlinks / hooks
oobo update                                  # check for updates and self-update
```

Tool detection is automatic on every commit. No indexing step required.

---

## Configuration

Most config is now declarative via `oobo settings`:

```bash
oobo settings set key sk_...                         # API key for remote search
oobo settings set remote https://oobo.mycompany.com  # self-hosted backend
oobo settings set transparency on                    # store redacted transcripts
oobo settings project set transparency off           # per-project override
oobo settings set setup.scan_roots "~/src,~/work"
```

For full fidelity or automation, `~/.oobo/config` still works:

```toml
[server]
url = "https://api.oobo.ai"
api_key = "sk_..."

[transparency]
mode = "off"           # off | on

[cursor]
enabled = true

[claude]
enabled = false
```

Full tool list: `cursor`, `claude`, `gemini`, `windsurf`, `aider`, `copilot`, `zed`, `trae`, `codex`, `opencode`, `kiro`, `continue`, `droid`, `junie`, `amp`.

---

## Remote & Self-Hosting

By default, oobo points at **`api.oobo.ai`** — our free hosted backend. Create a free account at [oobo.ai](https://oobo.ai), grab an API key, and run:

```bash
oobo settings set key <your_key>
```

That stores the key for authenticated API use (e.g. `oobo search --remote`). Team sync is Git-first — anchors live on the orphan branch and push with your code. Run `oobo settings unset key` to clear the key.

To run your own server:

```bash
oobo settings set remote https://oobo.mycompany.com
```

Your backend implements endpoints under `/anchors`:

| Endpoint           | Method | Auth            | Required | Purpose                          |
| ------------------ | ------ | --------------- | -------- | -------------------------------- |
| `/anchors/search`  | POST   | Bearer token    | **Yes**  | Search anchors/sessions          |
| `/anchors/health`  | GET    | None            | No       | Health check (connectivity test) |

---

## Build from Source

```bash
git clone https://github.com/ooboai/oobo.git && cd oobo
cargo build --release
# binary at target/release/oobo
```

---

## Privacy

- **Read-only** — never writes to AI tool directories
- **Local by default** — anchors live on a git orphan branch in your repo, config in `~/.oobo/`. Nothing leaves your machine unless you configure a remote
- **Secret redaction** — sessions are scrubbed with [gitleaks](https://github.com/gitleaks/gitleaks) patterns before sharing
- **No telemetry** — oobo does not phone home
- **Config protection** — API keys in config get `chmod 0600`

See [SECURITY.md](SECURITY.md) for the full policy.

---

## Contributing

Oobo is open source under [Apache 2.0](LICENSE-APACHE) and [MIT](LICENSE-MIT) (dual licensed, at your option). See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, project structure, and guidelines.

---

## License

Dual licensed under [Apache 2.0](LICENSE-APACHE) and [MIT](LICENSE-MIT), at your option.
