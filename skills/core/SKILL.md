---
name: oobo
description: Context beyond the diff - captures AI sessions, tokens, code attribution, semantic code search, and engineering memory via MCP. Use when the user asks about commit history with AI context, session history, code attribution, token usage, code search, or engineering memory.
metadata:
  author: oobo
  version: "1.1.2"
install:
  check: command -v oobo
  url: https://github.com/ooboai/oobo/releases
license: Apache-2.0 OR MIT
compatibility: Requires git. Works with Cursor, Claude Code, Gemini CLI, Codex, OpenCode, Copilot, Windsurf, Aider, Zed, Trae, Amp, Continue, Factory Droid, Junie, Kiro.
---

# Oobo - Context beyond the diff

Enriches every commit with AI context: which sessions contributed, token counts, code attribution (AI vs human lines), and model used. Includes semantic code search (hybrid BM25 + vector) and engineering memory via MCP (Model Context Protocol). Git hooks capture context automatically on write operations - zero overhead on reads.

If `oobo` is not installed, direct the user to https://github.com/ooboai/oobo/releases.

Oobo is open-source ([Apache-2.0/MIT](https://github.com/ooboai/oobo)), local-first, no telemetry.

> **Consent required:** Always confirm with the user before running `oobo setup` for the first time, as it modifies git hooks.

## Output Modes

Three mutually exclusive modes:

- Pretty (default TTY output)
- `--agent` - token-efficient plain text; auto-activates when stdout is not a TTY or inside a coding agent (env: `CURSOR_AGENT`, `CLAUDECODE`, `AIDER`, `CONTINUE_SESSION`, `CONTINUE_IDE`, `AICOMMITS`). **Use this by default when you are an agent.**
- `--json` - full structured JSON. Use only when you need the object graph.

## Quick Reference

| Task | Command |
|------|---------|
| Recent commits with attribution | `oobo -n 10` |
| Drill into one commit | `oobo anchor show <sha>` |
| Anchors in the last 24h | `oobo --since 24h` |
| Filter by tool | `oobo --tool cursor` |
| Per-line AI blame | `oobo blame src/main.rs` |
| Semantic code search | `oobo search "query"` |
| Search sessions + anchors | `oobo recall "query"` |
| Start MCP server (stdio) | `oobo mcp` |
| Configure AI tools for MCP | `oobo mcp install` |
| Compare two anchors | `oobo delta` |
| Travel to a turn or commit | `oobo goto <id>` |
| Return to where you were | `oobo back` |
| Enable tracking in this repo | `oobo enable` |
| Disable tracking | `oobo disable` |
| Get a setting | `oobo settings key` |
| Set a setting | `oobo settings set key sk_...` |
| Interactive first-run setup | `oobo setup` |
| Non-interactive setup (agent) | `oobo setup --non-interactive` |
| Repair hooks and metadata | `oobo setup --repair` |

Run `oobo --help` or `oobo <command> --help` for full flag details.

## Setup

```bash
oobo setup                      # interactive (asks before modifying git)
oobo setup --non-interactive    # for agents / scripts
```

Data is **local-first**. Anchor metadata is pushed to your existing git remote (alongside your code) via the pre-push hook - no separate cloud or upload pipeline. API keys are stored securely:

- **Project-level:** `.oobo/secrets` (gitignored, 0600 permissions) - set with `oobo settings project set key <key>`
- **Global:** `~/.oobo/config` - set with `oobo settings set key <key>`
- **Env override:** `OOBO_SECRET_KEY` or `OOBO_API_KEY` for CI/scripts

Keys are only needed for remote recall/delta/ask/get_context. Resolution order: env var > project secrets > global config.

Capture is real-time: git hooks (post-commit, pre-push, post-merge, post-rewrite) fire automatically and write anchors to the orphan branch. No background scanning or indexing needed.

## Key Behaviors

- Commit SHA prefix matching for `oobo anchor show` (unambiguous prefixes only)
- Token counts marked `is_estimated: true` are heuristic estimates; `false` means native counts from the tool
- `oobo update` self-updates and runs migrations automatically
- Data lives on a git orphan branch (`oobo/anchors/v1`) - git-native, no external database
- `oobo blame` is a strict superset of `git blame` - every flag is forwarded; machine-output formats (`--porcelain`, `--line-porcelain`, `--incremental`) bypass the AI overlay automatically.

## MCP (Model Context Protocol)

Oobo exposes engineering memory to AI agents via MCP. One command configures your tools:

```bash
oobo mcp install              # auto-detect Cursor/Claude/Copilot and configure
oobo mcp install cursor       # configure a specific tool
oobo mcp install --hosted     # cloud-only mode (no local binary needed by the tool)
oobo mcp install --remove     # uninstall MCP configuration
```

### Tools exposed

| Tool | Description |
|------|-------------|
| `search` | Semantic code search (local, via sonar) |
| `find_related` | Find code related to a query across the indexed codebase |
| `recall` | Search engineering memory (sessions, decisions, patterns) |
| `get_context` | Token-budgeted, file-scoped context from engineering history |
| `ask` | Natural language questions against the team's engineering memory |

**Agent best practice:** Call `get_context` at the start of any non-trivial task with the files you plan to modify. It returns plain-text guidance (not raw JSON) with relevant history, past decisions, and known pitfalls for those files.

The local server (`oobo mcp`) runs as a stdio child process managed by the AI tool. Cloud tools (`recall`, `get_context`, `ask`) require an API key (`oobo settings project set key <key>`).

## Legacy commands

0.1.x commands (`scan`, `sessions`, `projects`, `stats`, `card`, `share`, `sync`, `auth`, `ignore`, etc.) now print a migration hint and map to their 1.0 equivalent. The hints will be removed in 1.1.

## References

- [Commands](references/COMMANDS.md) - full command reference with all flags and examples
- [API Surface](references/API_SURFACE.md) - remote endpoints and agent lifecycle hooks
- [JSON Schema](references/JSON_SCHEMA.md) - `--json` field listings per command
- [Trust & Security](references/TRUST.md) - privacy policy, security details, data handling
