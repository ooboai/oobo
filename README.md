[License](LICENSE-APACHE)
[CI](https://github.com/ooboai/oobo/actions/workflows/ci.yml)
[Issues](https://github.com/ooboai/oobo/issues)
GitHub Release

# oobo

### Git for agents (and humans).

A transparent git decorator that enriches every commit with AI context: sessions, tokens, and code attribution. Humans use it like normal git. Agents use `--agent` for structured JSON. Anchor metadata always syncs via git; transcripts stay local unless you turn transparency on.

[How It Works](#how-it-works) · [Install](#installation) · [Quick Start](#quick-start) · [For AI Agents](#for-ai-agents) · [Contributing](CONTRIBUTING.md)

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

Each anchor records which AI sessions contributed, token counts, code attribution (AI vs human lines), model used, and session duration. Anchors live in a local SQLite database and on a git orphan branch (`oobo/anchors/v1`) that travels with the repo. The orphan branch is always created on the first intercepted commit — metadata syncs automatically.

### Supported tools


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
# 1. Run the setup wizard (detects your tools, configures remote)
oobo setup

# 2. (Optional) Log in for cloud sync — free at oobo.ai
oobo auth login --key <your_key>

# 3. Use oobo wherever you'd use git — everything passes through
oobo commit -m "fix auth middleware"
oobo push origin main

# 4. See what happened
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
oobo sessions search "auth bug"  # search by keyword
```

The TUI shows source, model, tokens, duration, and title for each session. Select one to scroll through the full conversation.

For scripting and automation, session IDs (UUIDs) are available in JSON output. You can use a short prefix to reference them:

```bash
oobo sessions list --agent             # get session IDs as JSON
oobo sessions list --tool claude -n 10 # filter by tool, limit results
oobo sessions show abc12def            # view by ID prefix
oobo sessions search "auth" --all      # search across all projects
oobo sessions export abc12def --format md --out chat.md
```

### Enriched commit history

```bash
oobo anchors                     # commit history with AI context, attribution
oobo anchors -n 20               # show last 20 commits
oobo a --agent                   # JSON output (short alias)
```

### Analytics

```bash
oobo stats                       # tokens, attribution, productivity
oobo stats --project myapp       # per-project
oobo stats --tool cursor         # per-tool
oobo stats --since 30d           # time-filtered
```

### Projects

```bash
oobo projects                    # interactive TUI for all tracked projects
oobo projects show myapp         # details + sessions for a project
oobo projects forget myapp       # remove a project from tracking
```

### Developer card

```bash
oobo card                        # generate your developer stats card (PNG)
oobo card --format svg           # SVG output
oobo card --format md            # markdown output
oobo card --out dev.png          # save to a custom path
```![oobo card](.github/oobo-card.png)

Generates an overview of your AI tool usage — sessions, tokens, models, AI code percentage, commit profile — and saves it as a shareable file. No project names or private data included.

### Sharing & exporting

```bash
oobo share <session_id>                # share a redacted session (uploads to server)
oobo share <session_id> --out chat.md  # save redacted session as markdown
oobo sessions export <id> --format md  # export full session as markdown
oobo sessions export <id> --format json --out chat.json
```

### Sync & transparency

```bash
oobo sync                        # show current sync status
oobo sync on                     # enable backend sync (global)
oobo sync off                    # disable backend sync
oobo sync --import               # import anchors from orphan branch into local DB
oobo transparency                # show current per-repo transparency setting
oobo transparency on             # sync redacted transcripts for this repo
oobo transparency off            # keep transcripts local only
oobo transparency reset          # clear per-repo override, use global default
```

### Auth

```bash
oobo auth login                  # log in to api.oobo.ai (free) or self-hosted
oobo auth login --key <key>      # authenticate with an API key
oobo auth status                 # show auth state + configured tool keys
oobo auth logout                 # remove credentials
oobo auth set-remote <url>       # point to a self-hosted server
oobo auth anthropic <key>        # set Anthropic Admin API key
oobo auth openai <key>           # set OpenAI API key
oobo auth copilot <token>        # set GitHub Copilot org PAT
oobo auth google <key>           # set Google AI Studio key
oobo auth windsurf <key>         # set Windsurf/Codeium service key
```

### Ignore & unignore

```bash
oobo ignore                      # stop tracking the current repo
oobo ignore --list               # show all ignored repos
oobo unignore                    # re-enable tracking for this repo
```

### Maintenance

```bash
oobo scan                        # discover projects + sessions from all tools
oobo index                       # compute token counts and analytics
oobo index --force               # re-index already indexed sessions
oobo index --bg                  # run indexing in background
oobo inspect --fix               # diagnose and auto-repair issues
oobo sources                     # data source status per tool
oobo update                      # check for updates and self-update
oobo update --check              # only check, don't install
```

---

## For AI Agents

Oobo is built for agents. Agents commit code constantly, across tools, often in parallel. Without oobo, there is no record of which agent wrote what, how many tokens it took, or which conversation produced a given function.

### The --agent flag

Every command supports `--agent` for structured JSON output:

```bash
oobo sessions --agent                  # JSON list of sessions
oobo sessions list --agent             # same (explicit subcommand)
oobo sessions list --all --tool claude --agent  # filter + JSON
oobo sessions show <id> --agent        # full conversation as JSON
oobo sessions search <q> --agent       # search results as JSON
oobo sessions export <id> --format json # export session as JSON
oobo projects --agent                  # JSON list of projects
oobo projects show <name> --agent      # project detail as JSON
oobo anchors --agent                   # enriched commit history as JSON
oobo stats --agent                     # analytics as structured data
oobo card --agent                      # developer card as JSON
oobo sources --agent                   # data source coverage as JSON
oobo dash --agent                      # configuration overview as JSON
oobo version --agent                   # version info as JSON
oobo inspect --agent                   # diagnostics as machine-readable JSON
oobo share <id> --agent                # redacted session as JSON
oobo scan --agent                      # suppresses interactive output
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
url = "https://api.oobo.ai"   # default — or your own server
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

## Remote & Self-Hosting

By default, oobo points at **`api.oobo.ai`** — our free hosted backend. Create a free account at [oobo.ai](https://oobo.ai), grab an API key, and run:

```bash
oobo auth login --key <your_key>
```

If you prefer to run your own server, point oobo at it:

```bash
oobo auth set-remote https://oobo.mycompany.com
```

Your backend implements endpoints under `/anchors`. Only **ingest** is required — the rest are optional:

| Endpoint           | Method | Auth            | Required | Purpose                          |
| ------------------ | ------ | --------------- | -------- | -------------------------------- |
| `/anchors/ingest`  | POST   | Bearer token    | **Yes**  | Accept anchor data from commits  |
| `/anchors/verify`  | GET    | Bearer token    | No       | Verify an API key is valid       |
| `/anchors/health`  | GET    | None            | No       | Health check (connectivity test) |
| `/anchors/share`   | POST   | Bearer optional | No       | Accept shared sessions           |

### Backend implementation

**`POST /anchors/ingest`** (required) — receives anchor data on every commit. The request body is JSON with the full anchor payload (commit metadata, AI attribution, linked sessions, and optionally redacted transcripts). Respond `200`/`202` on success. Return `409` for duplicate anchors (same `commit_hash` + `git_remote`). The CLI treats `409` as success silently.

**`GET /anchors/verify`** (optional) — validates the API key in the `Authorization: Bearer <key>` header. Return `200` with a JSON body (optionally including `{"email": "..."}`) on success, or `401` on failure. Called during `oobo auth login`.

**`GET /anchors/health`** (optional) — no auth required. Return any `2xx` response. Called by `oobo dash` to check connectivity.

**`POST /anchors/share`** (optional) — accepts a redacted session for sharing. Bearer token is optional — if present, the share is associated with the user's account; if absent, the share is anonymous. Return `200`/`201` with a `{"url": "..."}` body so the CLI can print the share link.

All authenticated requests include:

```
Authorization: Bearer <api_key>
User-Agent: oobo/<version>
Content-Type: application/json
```

See the [anchor payload reference](#the-anchor) and `src/remote/payload.rs` for the full schema.

---

## Privacy

- **Read-only** — never writes to AI tool directories
- **Local by default** — everything stays in `~/.oobo/`. Nothing leaves your machine unless you configure a remote server
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
