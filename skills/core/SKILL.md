---
name: oobo
description: Git decorator that gives every commit memory — links AI sessions, tokens, and code attribution to your commits. Use when the user asks about commit history with AI context, session history, code attribution, or token usage.
metadata:
  author: oobo
  version: "1.0.0-rc.1"
install:
  check: command -v oobo
  url: https://github.com/ooboai/oobo/releases
license: Apache-2.0 OR MIT
compatibility: Requires git. Works with Cursor, Claude Code, Gemini CLI, Codex, OpenCode, Copilot, Windsurf, Aider, Zed, Trae, Amp, Continue, Factory Droid, Junie, Kiro.
---

# Oobo — Git for agents (and humans)

Transparent git decorator that enriches every commit with AI context: which sessions contributed, token counts, code attribution (AI vs human lines), and model used. When aliased as `git`, all operations pass through transparently — read commands have zero overhead, write commands get enriched with anchor metadata.

If `oobo` is not installed, direct the user to https://github.com/ooboai/oobo/releases.

Oobo is open-source ([Apache-2.0/MIT](https://github.com/ooboai/oobo)), local-first, no telemetry.

> **Consent required:** Always confirm with the user before running `oobo setup` or `oobo alias install` for the first time, as these modify git configuration.

## Output Modes

Three mutually exclusive modes:

- Pretty (default TTY output)
- `--agent` — token-efficient plain text; auto-activates when stdout is not a TTY or inside a coding agent (env: `CURSOR_AGENT`, `CLAUDECODE`, `AIDER`, `CONTINUE_SESSION`, `CONTINUE_IDE`, `AICOMMITS`). **Use this by default when you are an agent.**
- `--json` — full structured JSON. Use only when you need the object graph.

## Quick Reference

| Task | Command |
|------|---------|
| Recent commits with attribution | `oobo anchors -n 10` |
| Drill into one commit | `oobo anchors show <sha>` |
| Anchors in the last 24h | `oobo anchors --since 24h` |
| Filter by tool | `oobo anchors --tool cursor` |
| Per-line AI blame | `oobo blame src/main.rs` |
| Plain git blame (no AI column) | `oobo blame --no-ai src/main.rs` |
| Search sessions + anchors | `oobo search "query"` |
| Enable tracking in this repo | `oobo enable` |
| Disable tracking (passthrough only) | `oobo disable` |
| Get a setting | `oobo settings key` |
| Set a setting | `oobo settings set key sk_...` |
| Install `git` alias | `oobo alias install` |
| Interactive first-run setup | `oobo setup` |
| Non-interactive setup (agent) | `oobo setup --non-interactive` |
| Force a reindex | `oobo setup --reindex` |

Run `oobo --help` or `oobo <command> --help` for full flag details.

## Setup

```bash
oobo setup                 # interactive (asks before modifying git)
oobo setup --non-interactive   # for agents / scripts
```

Sync is **off by default**. Nothing leaves the machine unless the user sets an API key (`oobo settings set key ...`).

Indexing is automatic: view commands kick a background rescan when `last_scanned_at` is older than 5 minutes. Opt out with `OOBO_NO_AUTO_INDEX=1`.

## Key Behaviors

- Commit SHA prefix matching for `oobo anchors show` (unambiguous prefixes only)
- Token counts marked `is_estimated: true` are tiktoken estimates; `false` means native from the tool
- `oobo update` self-updates and runs migrations automatically
- Data lives in `~/.oobo/db/oobo.db` (local SQLite)
- `oobo blame` is a strict superset of `git blame` — every flag is forwarded; machine-output formats (`--porcelain`, `--line-porcelain`, `--incremental`) bypass the AI overlay automatically.

## Legacy commands

0.1.x commands (`scan`, `sessions`, `projects`, `stats`, `card`, `share`, `sync`, `auth`, `ignore`, etc.) now print a migration hint and map to their 1.0 equivalent. The hints will be removed in 1.1.

## References

- [Commands](references/COMMANDS.md) — full command reference with all flags and examples
- [API Surface](references/API_SURFACE.md) — remote endpoints and agent lifecycle hooks
- [JSON Schema](references/JSON_SCHEMA.md) — `--json` field listings per command
- [Trust & Security](references/TRUST.md) — privacy policy, security details, data handling
