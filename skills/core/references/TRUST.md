# Trust & Security

Oobo is open-source software published by [NoCode Inc](https://nocode.inc).

| Property | Detail |
|----------|--------|
| Source code | Public at [github.com/ooboai/oobo](https://github.com/ooboai/oobo) — Apache 2.0 / MIT dual-licensed |
| Security policy | [SECURITY.md](https://github.com/ooboai/oobo/blob/main/SECURITY.md) — responsible disclosure, safe harbor |
| Privacy policy | [oobo.ai/privacy](https://oobo.ai/privacy) |
| Read-only | Never writes to AI tool directories — only reads session metadata (timestamps, model names, token counts) |
| Local-first | All data stays in `~/.oobo/`. Nothing leaves the machine unless the user explicitly configures sync |
| No telemetry | Does not phone home or collect usage data |
| No credentials | Never accesses API keys, browsing history, or file contents outside of AI tool storage |
| Secret redaction | Sessions scrubbed with [gitleaks](https://github.com/gitleaks/gitleaks) patterns before any sharing |
| Install | Downloads a pre-built binary from GitHub Releases — no arbitrary code execution |
| No sudo required | Installs to `~/.oobo/bin`, no elevated permissions needed |

## Supported Tools

Cursor, Claude Code, Gemini CLI, Codex CLI, OpenCode, GitHub Copilot Chat, Windsurf, Aider, Zed AI, Trae.

## Architecture

Oobo operates in two layers:

- **Level 1 (Git Decorator)**: Decorates git to intercept commits, enrich them with anchor metadata (linked sessions, agent/human attribution, diff stats), and sync metadata via the `oobo/anchors/v1` orphan branch.
- **Level 2 (Local Memory Engine)**: Maintains a local SQLite database with session history, token counts, analytics, and time-series data across all AI tools.

## Agent Configuration

Agents should operate with **transparency on** — this is the recommended default for all automated workflows. Transparency mode writes anchor metadata and redacted transcripts to a local orphan branch (`oobo/anchors/v1`), giving teams visibility into AI contributions.

| Setting | Value | Why |
|---------|-------|-----|
| `transparency.mode` | `on` | Metadata + redacted transcripts sync |
| `git.alias_enabled` | `true` | Automatic enrichment on every commit |
| `server.sync` | `false` | Off by default — user opts in via `oobo sync on` |
| `--agent` flag | Always use | Compact output, low token cost |
| `--json` flag | When needed | Full structured data for parsing |
