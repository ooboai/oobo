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

- **Drop-In Git Replacement** — Use `oobo` exactly like `git`. Every command passes through transparently. Read operations have zero overhead.
- **AI Session Tracking** — Automatically discovers and links AI chat sessions to your commits — which agent wrote what, how many tokens it took, and which conversation produced each change.
- **15 Tools Supported** — Cursor, Claude Code, Gemini CLI, OpenCode, Codex, Aider, GitHub Copilot, Windsurf, Zed, Trae, Amp, Continue, Factory Droid, Junie, and Kiro.
- **Code Attribution** — Know exactly which lines were AI-generated vs human-written, per commit.
- **Agent-Native** — Three output modes (pretty / `--agent` token-efficient plain text / `--json` structured). `--agent` auto-activates when stdout isn't a TTY or inside a coding agent.
- **Local-First, Private by Default** — Everything stays in `~/.oobo/`. Nothing leaves your machine unless you opt in. No telemetry. Secrets are redacted before sharing.
- **Anchor System** — Extends git commits with structured AI metadata that travels with the repo via a git orphan branch. No external dependencies.

---

## Quick Start

```bash
# 1. Install — setup runs automatically, detects your tools, configures everything
curl -fsSL https://oobo.ai/install.sh | bash

# 2. Use oobo wherever you'd use git — everything passes through
oobo commit -m "fix auth middleware"
oobo push origin main

# 3. See what happened
oobo                      # in-repo: scrollable feed of your anchors
oobo anchors              # enriched commit history with AI context
oobo anchors show <sha>   # drill into one anchor (sessions, tokens, attribution)
oobo blame src/main.rs    # git blame + per-line AI attribution
oobo search "auth bug"    # search sessions + anchors across projects
```

**Optionally**, alias `git` so you don't have to think about it:

```bash
oobo alias install      # adds alias git=oobo to your shell rc
```

**Optionally**, connect to [oobo.ai](https://oobo.ai) for free cloud sync:

```bash
oobo settings set key <your_key>
```

---

## How It Works

```
You run:  oobo commit -m "fix auth middleware"

  1. Execute real `git commit`
  2. Detect write operation
  3. Read AI sessions from local tool storage
  4. Build anchor: commit + sessions + tokens + attribution
  5. Write anchor to local DB + git orphan branch
  6. POST anchor to remote (if configured) → /anchors/ingest
  7. Return git's exit code unchanged
```

Read operations (`status`, `log`, `diff`, ...) pass straight through to git with zero overhead.

### The anchor

An **anchor** is oobo's core primitive — it extends a git commit with AI context:

```
Git:   commit = diff(files)
Oobo:  anchor = commit + sessions + tokens + attribution
```

Each anchor records which AI sessions contributed, token counts, code attribution (AI vs human lines), model used, and session duration. Anchors live in a local SQLite database and on a git orphan branch (`oobo/anchors/v1`) that travels with the repo.

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
oobo anchors --agent           # token-efficient commit feed
oobo anchors --json            # flat JSON array of anchors
oobo blame src/main.rs --json  # per-line AI attribution as JSON
oobo search "auth" --agent     # compact search results
```

### Skill file

Oobo installs a skill file at `~/.oobo/skills/oobo/SKILL.md` during `oobo setup`, with symlinks in `~/.agents/skills/oobo/`, `~/.claude/skills/oobo/`, `~/.codex/skills/oobo/`, `~/.cursor/skills/oobo/`, and `~/.gemini/skills/oobo/`. AI coding tools discover the skill automatically and know how to install and use oobo.

### Agent lifecycle hooks

For tools that support it (Cursor, Claude Code, Gemini CLI, OpenCode, Kiro, Continue, Factory Droid), oobo installs hooks that track agent activity in real time: session start/end, tool calls, subagent spawns, thinking events, and context compaction. This enables precise session linking and rich telemetry attached to every commit anchor.

---

## Commands

### Bare `oobo`

```bash
oobo                # in-repo + TTY: scrollable anchor-feed TUI
                    # in-repo + --agent/--json: same as `oobo anchors`
                    # outside a repo: first-run hint or short status
```

### Anchors — enriched commit history

```bash
oobo anchors                           # last 50 anchors (pretty)
oobo anchors -n 20 --since 7d          # filtered
oobo anchors --tool cursor             # per-tool
oobo anchors --project myapp           # outside a repo, aggregate one project
oobo anchors show <sha>                # drill-down: sessions, tokens, attribution
oobo anchors show <sha> --json         # structured JSON for scripts
```

### Blame — git blame + AI attribution

```bash
oobo blame src/main.rs                 # git blame with an extra AI column
oobo blame src/main.rs @abc123         # at a specific commit
oobo blame --no-ai src/main.rs         # byte-identical to `git blame`
oobo blame src/main.rs --json          # per-line AI attribution as JSON
```

Every `git blame` flag (`-L`, `-w`, `--porcelain`, etc.) is forwarded; machine-output formats (`--porcelain`, `--line-porcelain`, `--incremental`) bypass the AI column automatically.

### Search — find sessions + anchors

```bash
oobo search "auth bug"                 # full-text search
oobo search "auth" --since 7d --tool claude --project myapp
oobo search "auth" --json              # structured results
```

### Settings — declarative per-scope config

```bash
oobo settings                          # list default-scope keys
oobo settings key                      # get the API key (default scope)
oobo settings set key sk_...           # set on default scope
oobo settings myrepo set remote https://oobo.mycompany.com
oobo settings unset transparency       # remove a key
```

Scopes: `default` (implicit), `system`, or any project name. Verbs: `get` (default), `set`, `unset`.

### Per-project toggles

```bash
oobo enable                            # start tracking this repo
oobo disable                           # stop (commits still pass through to git)
```

### Alias

```bash
oobo alias install                     # add `alias git=oobo` to your shell rc
oobo alias uninstall                   # remove it
```

### Setup & maintenance

```bash
oobo setup                             # interactive wizard: install hooks, discover tools, seed config
oobo setup --non-interactive           # for scripts + first-run agents
oobo setup --reindex                   # forced full rescan
oobo setup --repair                    # fix broken symlinks / hooks
oobo update                            # check for updates and self-update
```

Indexing is automatic: view commands kick a background rescan when `last_scanned_at` is older than 5 minutes. Opt out with `OOBO_NO_AUTO_INDEX=1`.

### Git passthrough

Any verb not recognized by oobo is forwarded to `git` unchanged:

```bash
oobo status                            # → git status
oobo commit -m "fix"                   # → git commit + writes an anchor
oobo push origin main                  # → git push
```

---

## Configuration

Most config is now declarative via `oobo settings`:

```bash
oobo settings set key sk_...                        # api key (default scope)
oobo settings set remote https://oobo.mycompany.com # self-hosted backend
oobo settings set transparency on                   # sync redacted transcripts for this repo
oobo settings myrepo set transparency off           # per-project override
oobo settings system set setup.scan_roots "~/src:~/work"
```

For full fidelity or automation, `~/.oobo/config.toml` still works:

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

To run your own server:

```bash
oobo settings set remote https://oobo.mycompany.com
```

Your backend implements endpoints under `/anchors`. Only **ingest** is required:

| Endpoint           | Method | Auth            | Required | Purpose                          |
| ------------------ | ------ | --------------- | -------- | -------------------------------- |
| `/anchors/ingest`  | POST   | Bearer token    | **Yes**  | Accept anchor data from commits  |
| `/anchors/verify`  | GET    | Bearer token    | No       | Verify an API key is valid       |
| `/anchors/health`  | GET    | None            | No       | Health check (connectivity test) |
| `/anchors/share`   | POST   | Bearer optional | No       | Accept shared sessions           |

See `src/remote/payload.rs` for the full anchor payload schema.

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
- **Local by default** — everything stays in `~/.oobo/`. Nothing leaves your machine unless you configure a remote
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
