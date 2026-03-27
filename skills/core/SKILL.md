---
name: oobo
description: Git decorator that gives every commit memory — links AI sessions, tokens, and code attribution to your commits. Use when the user asks about commit history with AI context, session history, code attribution, token usage, or generating a developer card.
metadata:
  author: oobo
  version: "0.1.14"
install:
  check: command -v oobo
  url: https://github.com/ooboai/oobo/releases
license: Apache-2.0 OR MIT
compatibility: Requires git. Works with Cursor, Claude Code, Gemini CLI, Codex, OpenCode, Copilot, Windsurf, Aider, Zed, Trae.
---

# Oobo — Git for agents (and humans)

Transparent git decorator that enriches every commit with AI context: which sessions contributed, token counts, code attribution (AI vs human lines), and model used. When aliased as `git`, all operations pass through transparently — read commands have zero overhead, write commands get enriched with anchor metadata.

If `oobo` is not installed, direct the user to https://github.com/ooboai/oobo/releases.

Oobo is open-source ([Apache-2.0/MIT](https://github.com/ooboai/oobo)), local-first, no telemetry.

> **Consent required:** Always confirm with the user before running `oobo setup` or `oobo alias install` for the first time, as these modify the git configuration.

## Output Modes

All commands support `--agent` (compact, low-token) and `--json` (full structured). Always prefer `--agent`. Use `--json` only when you need the full object graph.

## Quick Reference

| Task | Command |
|------|---------|
| Recent commits with attribution | `oobo anchors -n 10` |
| Per-line AI blame for a file | `oobo blame src/main.rs` |
| List sessions (current project) | `oobo sessions` |
| List sessions (all projects) | `oobo sessions list --all` |
| Show session details | `oobo sessions show <id>` |
| Search sessions | `oobo sessions search "query" --all` |
| Project overview | `oobo projects` |
| Global stats | `oobo stats` |
| Stats by project or tool | `oobo stats --project <name>` |
| Developer card | `oobo card` |
| Share a session | `oobo share <id>` |
| Diagnostics / auto-fix | `oobo inspect --fix` |
| Sync status | `oobo sync` |

Run `oobo --help` or `oobo <command> --help` for full flag details.

## Setup

```bash
oobo setup        # Configure transparency, git alias, hooks (asks user first)
oobo scan         # Discover projects and sessions
```

Sync is **off by default**. Nothing leaves the machine unless the user runs `oobo sync on` and configures an API key.

## Key Behaviors

- Session IDs support prefix matching (e.g. `2c97` matches `2c97dced-3950-...`)
- Token counts marked `is_estimated: true` are tiktoken estimates; `false` means native from the tool
- `oobo update` self-updates and runs migrations automatically
- Data lives in `~/.oobo/db/oobo.db` (local SQLite)

## References

- [Commands](references/COMMANDS.md) — full command reference with all flags and examples
- [API Surface](references/API_SURFACE.md) — remote endpoints and agent lifecycle hooks
- [JSON Schema](references/JSON_SCHEMA.md) — `--json` field listings per command
- [Trust & Security](references/TRUST.md) — privacy policy, security details, data handling
