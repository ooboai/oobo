# Trust & Security

Oobo is open-source software published by [NoCode Inc](https://nocode.inc).

| Property | Detail |
|----------|--------|
| Source code | Public at [github.com/ooboai/oobo](https://github.com/ooboai/oobo) — Apache 2.0 / MIT dual-licensed |
| Security policy | [SECURITY.md](https://github.com/ooboai/oobo/blob/main/SECURITY.md) — responsible disclosure, safe harbor |
| Privacy policy | [oobo.ai/privacy](https://oobo.ai/privacy) |
| Read-only | Never writes to AI tool directories — only reads session metadata (timestamps, model names, token counts) |
| Local-first | All data stays in `~/.oobo/` and the local orphan branch. Anchor metadata is pushed only to your existing git remote (alongside your code) via the pre-push hook. The optional remote API (search, delta) requires a separate API key. |
| No telemetry | Does not phone home or collect usage data |
| No credentials | Never accesses API keys, browsing history, or file contents outside of AI tool storage |
| Secret redaction | Sessions scrubbed with [gitleaks](https://github.com/gitleaks/gitleaks) patterns before any sharing |
| Install | Downloads a pre-built binary from GitHub Releases — no arbitrary code execution |
| No sudo required | Installs to `~/.oobo/bin`, no elevated permissions needed |

## Supported Tools

Cursor, Claude Code, Gemini CLI, Codex CLI, OpenCode, GitHub Copilot Chat, Windsurf, Aider, Zed AI, Trae, Amp, Continue, Factory Droid, Junie, Kiro.

## Architecture

Oobo is a git decorator that intercepts commits, enriches them with anchor metadata (linked sessions, agent/human attribution, diff stats, token counts), and stores everything on the `oobo/anchors/v1` orphan branch. Anchor data is git-native; a lightweight local SQLite cache under `~/.oobo/` accelerates lookups but is fully rebuildable from git.

## Agent Configuration

Agents should operate with **transparency on** — this is the recommended default for all automated workflows. Transparency mode writes anchor metadata and redacted transcripts to a local orphan branch (`oobo/anchors/v1`), giving teams visibility into AI contributions.

| Setting | Value | Why |
|---------|-------|-----|
| `transparency` | `on` | Metadata + redacted transcripts sync |
| `key` | *(optional)* | For remote search and delta only; no cloud upload pipeline exists |
| `--agent` flag | Always use | Compact output, low token cost |
| `--json` flag | When needed | Full structured data for parsing |
