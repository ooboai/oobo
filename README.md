[![License](https://img.shields.io/badge/License-Apache_2.0_or_MIT-blue)](LICENSE-APACHE)
[![CI](https://img.shields.io/github/actions/workflow/status/ooboai/oobo/ci.yml?label=CI)](https://github.com/ooboai/oobo/actions/workflows/ci.yml)
[![Issues](https://img.shields.io/github/issues/ooboai/oobo)](https://github.com/ooboai/oobo/issues)
![GitHub Release](https://img.shields.io/github/v/release/ooboai/oobo)

<h1 align="center">oobo</h1>

<h3 align="center">Git for agents (and humans).</h3>

<p align="center">
  A transparent git decorator that enriches every commit with AI context: sessions, tokens, and code attribution. Humans use it like normal git. Agents use <code>--agent</code> for structured JSON. Anchor metadata always syncs via git; transcripts stay local unless you turn transparency on.
</p>

<p align="center">
  <a href="#how-it-works">How It Works</a> ·
  <a href="#installation">Install</a> ·
  <a href="#quick-start">Quick Start</a> ·
  <a href="#for-ai-agents">For AI Agents</a> ·
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

---

## Why oobo?

AI coding tools generate conversations, token usage, and context that disappear the moment you close the tab. Meanwhile, `git log` only shows diffs — it has no idea that Claude wrote that function or that you spent 14k tokens debugging a race condition.

**oobo bridges the gap.** It decorates git transparently, and on every write operation (commit, push, merge) it captures the AI sessions that contributed to the change and writes structured metadata called an **anchor**.

No workflow changes. No plugins. No cloud required.

---

## How It Works

```
You run:  oobo commit -m "fix auth middleware"

  1. Execute real `git commit`
  2. Detect write operation
  3. Read AI sessions from local tool storage
  4. Build anchor: commit + sessions + tokens + attribution
  5. Write anchor to local DB + git orphan branch
  6. Fire event to endpoint (if configured)
  7. Return git's exit code unchanged
```

Read operations (`status`, `log`, `diff`, ...) pass straight through to git with zero overhead.

### The anchor

An **anchor** is oobo's core primitive — it extends a git commit with AI context:

```
Git:   commit = diff(files)
Oobo:  anchor = commit + sessions + tokens + attribution
```

Each anchor records which AI sessions contributed, token counts, code attribution (AI vs human lines), model used, and session duration. Anchors live in a local SQLite database and on a git orphan branch (`oobo/anchors/v1`) that travels with the repo. The orphan branch is always created on the first intercepted commit — metadata syncs automatically.

### Supported tools

| Tool | Sessions | Transcripts | Token Stats | Agent Hooks |
|------|:--------:|:-----------:|:-----------:|:-----------:|
| Cursor | ✓ | ✓ | ✓ | ✓ |
| Claude Code | ✓ | ✓ | ✓ | ✓ |
| Gemini CLI | ✓ | ✓ | ✓ | ✓ |
| OpenCode | ✓ | ✓ | ✓ | ✓ |
| Codex CLI | ✓ | ✓ | ✓ | — |
| Aider | ✓ | ✓ | ✓ | — |
| GitHub Copilot Chat | ✓ | ✓ | ✓ | — |
| Windsurf | ✓ | ✓ | partial | — |
| Zed | ✓ | ✓ | ✓ | — |
| Trae | ✓ | ✓ | partial | — |

All tools are read-only — oobo never writes to AI tool data directories.

---

## Installation

**Humans:**

```bash
curl -fsSL https://oobo.ai/install.sh | bash
```

**Agents** (silent install, JSON output):

```bash
curl -fsSL https://oobo.ai/install.sh | bash -s -- --agent
# → {"status":"ok","version":"...","binary":"...","platform":"..."}
```

Agents that read `~/.agents/skills/oobo/SKILL.md` will find the install command automatically in the frontmatter. The skill file is installed during `oobo setup`.

Or grab a binary from [Releases](https://github.com/ooboai/oobo/releases).

**Platforms:** macOS (Apple Silicon, Intel) · Linux (x86_64, ARM64, glibc, musl)

### Build from source

```bash
git clone https://github.com/ooboai/oobo.git && cd oobo
cargo build --release
# binary at target/release/oobo
```

---

## Quick Start

```bash
# 1. Run the setup wizard (detects your tools, configures endpoint)
oobo setup

# 2. Use oobo wherever you'd use git — everything passes through
oobo commit -m "fix auth middleware"
oobo push origin main

# 3. See what happened
oobo anchors       # enriched commit history with AI context
oobo sessions      # browse your AI chat sessions
oobo stats         # token usage, attribution
```

**Optionally**, alias `git` itself so you don't have to think about it:

```bash
oobo alias install      # adds alias git=oobo to your shell rc
```

### Browsing sessions

```bash
oobo sessions                    # interactive TUI — navigate with arrows, Enter to view
oobo sessions --all              # sessions across all projects
oobo sessions search "auth bug"  # search by keyword (shows IDs in output)
```

The TUI shows source, model, tokens, duration, and title for each session. Select one to scroll through the full conversation.

For scripting and automation, session IDs (UUIDs) are available in JSON output. You can then use a short prefix to reference them:

```bash
oobo sessions list --agent       # get session IDs as JSON
oobo sessions show abc12def      # view by ID prefix
oobo sessions export abc12def --format md --out chat.md
```

### Analytics

```bash
oobo stats                       # tokens, attribution, productivity
oobo stats --project myapp       # per-project
oobo stats --tool cursor         # per-tool
oobo stats --since 30d           # time-filtered
```

### Developer card

```bash
oobo card                        # generate your AI-first developer stats card
oobo card --out dev.md           # save to a custom path
```

Generates an overview of your AI tool usage — sessions, tokens, models, AI code percentage, commit profile — and saves it as a shareable markdown file. No project names or private data included.

### Maintenance

```bash
oobo scan                        # discover projects + sessions from all tools
oobo index                       # compute token counts and analytics
oobo inspect --fix               # diagnose and auto-repair issues
oobo sources                     # data source status per tool
oobo update                      # check for updates
```

---

## For AI Agents

Oobo is built for agents. Agents commit code constantly, across tools, often in parallel. Without oobo, there is no record of which agent wrote what, how many tokens it took, or which conversation produced a given function.

### The --agent flag

Every command supports `--agent` for structured JSON output:

```bash
oobo sessions --agent            # JSON list of sessions
oobo sessions list --agent       # same (explicit subcommand)
oobo sessions show <id> --agent  # full conversation as JSON
oobo sessions search <q> --agent # search results as JSON
oobo projects --agent            # JSON list of projects
oobo projects show <n> --agent   # project detail as JSON
oobo anchors --agent             # enriched commit history as JSON
oobo stats --agent               # analytics as structured data
oobo card --agent                # developer card as JSON
oobo sources --agent             # data source coverage as JSON
oobo dash --agent                # configuration overview as JSON
oobo version --agent             # version info as JSON
oobo inspect --agent             # diagnostics as machine-readable JSON
oobo share <id> --agent          # shared session as JSON
oobo scan --agent                # suppresses interactive output
```

`--agent` is a global flag. It works with any command at any position.

### Installing from an agent

```bash
curl -fsSL https://oobo.ai/install.sh | bash -s -- --agent
# → {"status":"ok","version":"...","binary":"...","platform":"..."}
```

The `--agent` flag on the installer suppresses colors and interactive prompts and returns a single JSON line. This is what agents should use.

### Skill file

Oobo installs a skill file at `~/.agents/skills/oobo/SKILL.md` during `oobo setup`. AI coding tools (Cursor, Claude Code, Codex, Gemini CLI, OpenCode) scan this path for skills. The skill file tells agents:

- How to check if oobo is installed (`command -v oobo`)
- How to install it (`curl -fsSL https://oobo.ai/install.sh | bash -s -- --agent`)
- Every available command with JSON field descriptions
- Recommended configuration for agent workflows

To print the skill file:

```bash
oobo agent
```

### Agent lifecycle hooks

For tools that support it (Cursor, Claude Code, Gemini CLI, OpenCode), oobo installs hooks that track when agent sessions start and end. This enables real-time session linking during commits, rather than relying on time-window correlation.

---

## Configuration

`oobo setup` runs an interactive wizard. Or edit `~/.oobo/config.toml` directly:

```toml
[server]
url = "https://your-endpoint.example.com"
api_key = "sk_..."

[transparency]
mode = "off"           # off | on
```

Toggle tools individually:

```toml
[cursor]
enabled = true

[claude]
enabled = false
```

Full list: `cursor`, `claude`, `gemini`, `windsurf`, `aider`, `copilot`, `zed`, `trae`, `codex`, `opencode`.

---

## Privacy

- **Read-only** — never writes to AI tool directories
- **Local by default** — everything stays in `~/.oobo/`. Nothing leaves your machine unless you configure an endpoint
- **Secret redaction** — sessions are scrubbed with [gitleaks](https://github.com/gitleaks/gitleaks) patterns before any sharing
- **No telemetry** — oobo does not phone home
- **Config protection** — API keys in config get `chmod 0600`

See [SECURITY.md](SECURITY.md) for the full policy.

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, project structure, and guidelines.

---

## License

Dual licensed under [Apache 2.0](LICENSE-APACHE) and [MIT](LICENSE-MIT), at your option.
