<div align="center">

<img src="assets/icon.png" alt="anchor" width="120" />

# anchor

### Git for agents (and humans).

A transparent git decorator that enriches every commit with AI context:<br/>sessions, tokens, and code attribution — across 15 AI coding tools.<br/>No workflow changes. No plugins. No cloud required.

[![License](https://img.shields.io/badge/license-Apache%202.0%20%2F%20MIT-blue)](LICENSE-APACHE)
[![CI](https://img.shields.io/github/actions/workflow/status/ooboai/anchor/ci.yml?label=CI)](https://github.com/ooboai/anchor/actions/workflows/ci.yml)
[![GitHub Release](https://img.shields.io/github/v/release/ooboai/anchor?color=green)](https://github.com/ooboai/anchor/releases)
[![Issues](https://img.shields.io/github/issues/ooboai/anchor)](https://github.com/ooboai/anchor/issues)
[![Stars](https://img.shields.io/github/stars/ooboai/anchor?style=social)](https://github.com/ooboai/anchor)

[Documentation](https://docs.oobo.ai) · [Report Bug](https://github.com/ooboai/anchor/issues/new?labels=bug) · [Request Feature](https://github.com/ooboai/anchor/issues/new?labels=enhancement) · [Contributing](CONTRIBUTING.md)

</div>

---

## Installation

```bash
curl -fsSL https://oobo.ai/install.sh | bash
```

**Platforms:** macOS (Apple Silicon, Intel) · Linux (x86_64, ARM64, glibc, musl)

Or grab a binary from [Releases](https://github.com/ooboai/anchor/releases).

---

## Features

- **Drop-In Git Replacement** — Use `anchor` exactly like `git`. Every command passes through transparently. Read operations have zero overhead.
- **AI Session Tracking** — Automatically discovers and links AI chat sessions to your commits — which agent wrote what, how many tokens it took, and which conversation produced each change.
- **15 Tools Supported** — Cursor, Claude Code, Gemini CLI, OpenCode, Codex, Aider, GitHub Copilot, Windsurf, Zed, Trae, Amp, Continue, Factory Droid, Junie, and Kiro.
- **Code Attribution** — Know exactly which lines were AI-generated vs human-written, per commit.
- **Agent-Native** — Three output modes (pretty / `--agent` token-efficient plain text / `--json` structured). `--agent` auto-activates when stdout isn't a TTY or inside a coding agent.
- **Local-First, Private by Default** — Everything lives in your git repo (orphan branch) and a small `~/.oobo/` config dir. No database. Nothing leaves your machine unless you opt in. No telemetry. Secrets are redacted before sharing.
- **Anchor System** — Extends git commits with structured AI metadata that travels with the repo via a git orphan branch. No external dependencies.

---

## Quick Start

```bash
# 1. Install — setup runs automatically, detects your tools, configures everything
curl -fsSL https://oobo.ai/install.sh | bash

# 2. Use anchor wherever you'd use git — everything passes through
anchor commit -m "fix auth middleware"
anchor push origin main

# 3. See what happened
anchor                      # in-repo: scrollable feed of your anchors
anchor anchors              # enriched commit history with AI context
anchor anchors show <sha>   # drill into one anchor (sessions, tokens, attribution)
anchor blame src/main.rs    # git blame + per-line AI attribution
anchor search "auth bug"    # search sessions + anchors across projects
```

**Optionally**, alias `git` so you don't have to think about it:

```bash
anchor alias install      # adds alias git=anchor to your shell rc
```

**Optionally**, add an [oobo.ai](https://oobo.ai) API key for **remote search** (and other authenticated API calls). Anchor metadata still syncs through Git by default.

```bash
anchor settings set key <your_key>
```

---

## How It Works

```
You run:  anchor commit -m "fix auth middleware"

  1. Execute real `git commit`
  2. Detect write operation
  3. Read AI sessions from local tool storage
  4. Build anchor: commit + sessions + tokens + attribution
  5. Write anchor to git orphan branch
  6. Return git's exit code unchanged
```

Read operations (`status`, `log`, `diff`, ...) pass straight through to git with zero overhead.

### The anchor

An **anchor** is anchor's core primitive — it extends a git commit with AI context:

```
Git:   commit = diff(files)
Anchor:  anchor = commit + sessions + tokens + attribution
```

Each anchor records which AI sessions contributed, token counts, code attribution (AI vs human lines), model used, and session duration. Anchors live on a git orphan branch (`oobo/anchors/v1`) that travels with the repo — no database, no external dependencies.

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

All tools are read-only — anchor never writes to AI tool data directories.

---

## For AI Agents

Anchor is built for agents. Agents commit code constantly, across tools, often in parallel. Without anchor, there is no record of which agent wrote what, how many tokens it took, or which conversation produced a given function.

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
anchor anchors --agent           # token-efficient commit feed
anchor anchors --json            # flat JSON array of anchors
anchor blame src/main.rs --json  # per-line AI attribution as JSON
anchor search "auth" --agent     # compact search results
```

### Skill file

Anchor installs a skill file at `~/.oobo/skills/oobo/SKILL.md` during `anchor setup`, with symlinks in `~/.agents/skills/oobo/`, `~/.claude/skills/oobo/`, `~/.codex/skills/oobo/`, `~/.cursor/skills/oobo/`, and `~/.gemini/skills/oobo/`. AI coding tools discover the skill automatically and know how to install and use anchor.

### Agent lifecycle hooks

For tools that support it (Cursor, Claude Code, Gemini CLI, OpenCode, Kiro, Continue, Factory Droid), anchor installs hooks that track agent activity in real time: session start/end, tool calls, subagent spawns, thinking events, and context compaction. This enables precise session linking and rich telemetry attached to every commit anchor.

---

## Commands

### Bare `anchor`

```bash
anchor                # in-repo + TTY: scrollable anchor-feed TUI
                    # in-repo + --agent/--json: same as `anchor anchors`
                    # outside a repo: first-run hint or short status
```

### Anchors — enriched commit history

```bash
anchor anchors                           # last 50 anchors (pretty)
anchor anchors -n 20 --since 7d          # filtered
anchor anchors --tool cursor             # per-tool
anchor anchors --project myapp           # outside a repo, aggregate one project
anchor anchors show <sha>                # drill-down: sessions, tokens, attribution
anchor anchors show <sha> --json         # structured JSON for scripts
```

### Blame — git blame + AI attribution

```bash
anchor blame src/main.rs                 # git blame with an extra AI column
anchor blame src/main.rs @abc123         # at a specific commit
anchor blame --no-ai src/main.rs         # byte-identical to `git blame`
anchor blame src/main.rs --json          # per-line AI attribution as JSON
```

Every `git blame` flag (`-L`, `-w`, `--porcelain`, etc.) is forwarded; machine-output formats (`--porcelain`, `--line-porcelain`, `--incremental`) bypass the AI column automatically.

### Search — find sessions + anchors

```bash
anchor search "auth bug"                 # full-text search
anchor search "auth" --since 7d --tool claude --project myapp
anchor search "auth" --json              # structured results
```

### Settings — declarative per-scope config

```bash
anchor settings                          # list default-scope keys
anchor settings key                      # get the API key (default scope)
anchor settings set key sk_...           # set API key (remote search)
anchor settings set remote https://oobo.mycompany.com
anchor settings project set remote oobo  # push anchor branch to git remote "oobo"
anchor settings unset transparency       # remove a key
```

Scopes: `default` (implicit) or `project`. Verbs: `get` (default), `set`, `unset`.

### Per-project toggles

```bash
anchor enable                            # start tracking this repo
anchor disable                           # stop (commits still pass through to git)
```

### Alias

```bash
anchor alias install                     # add `alias git=anchor` to your shell rc
anchor alias uninstall                   # remove it
```

### Setup & maintenance

```bash
anchor setup                             # interactive wizard: install hooks, discover tools, seed config
anchor setup --non-interactive           # for scripts + first-run agents
anchor setup --reindex                   # forced full rescan
anchor setup --repair                    # fix broken symlinks / hooks
anchor update                            # check for updates and self-update
```

Tool detection is automatic on every commit. No indexing step required.

### Git passthrough

Any verb not recognized by anchor is forwarded to `git` unchanged:

```bash
anchor status                            # → git status
anchor commit -m "fix"                   # → git commit + writes an anchor
anchor push origin main                  # → git push
```

---

## Configuration

Most config is now declarative via `anchor settings`:

```bash
anchor settings set key sk_...                        # API key for remote search
anchor settings set remote https://oobo.mycompany.com # self-hosted backend
anchor settings set transparency on                   # store redacted transcripts
anchor settings project set transparency off          # per-project override
anchor settings set setup.scan_roots "~/src,~/work"
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

By default, anchor points at **`api.oobo.ai`** — our free hosted backend. Create a free account at [oobo.ai](https://oobo.ai), grab an API key, and run:

```bash
anchor settings set key <your_key>
```

That stores the key for authenticated API use (e.g. `anchor search --remote`). Team sync is Git-first — anchors live on the orphan branch and push with your code. Run `anchor settings unset key` to clear the key.

To run your own server:

```bash
anchor settings set remote https://oobo.mycompany.com
```

Your backend implements endpoints under `/anchors`:

| Endpoint           | Method | Auth            | Required | Purpose                          |
| ------------------ | ------ | --------------- | -------- | -------------------------------- |
| `/anchors/search`  | POST   | Bearer token    | **Yes**  | Search anchors/sessions          |
| `/anchors/health`  | GET    | None            | No       | Health check (connectivity test) |

---

## Build from Source

```bash
git clone https://github.com/ooboai/anchor.git && cd anchor
cargo build --release
# binary at target/release/anchor
```

---

## Privacy

- **Read-only** — never writes to AI tool directories
- **Local by default** — anchors live on a git orphan branch in your repo, config in `~/.oobo/`. Nothing leaves your machine unless you configure a remote
- **Secret redaction** — sessions are scrubbed with [gitleaks](https://github.com/gitleaks/gitleaks) patterns before sharing
- **No telemetry** — anchor does not phone home
- **Config protection** — API keys in config get `chmod 0600`

See [SECURITY.md](SECURITY.md) for the full policy.

---

## Contributing

Anchor is open source under [Apache 2.0](LICENSE-APACHE) and [MIT](LICENSE-MIT) (dual licensed, at your option). See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, project structure, and guidelines.

---

## License

Dual licensed under [Apache 2.0](LICENSE-APACHE) and [MIT](LICENSE-MIT), at your option.
