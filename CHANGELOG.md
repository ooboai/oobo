# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-03-09

### Added

- **Transparent git decorator** — use `oobo` as a drop-in replacement for `git`, or install as a shell alias
- **AI session discovery** across 10 tools: Cursor, Claude Code, Gemini CLI, OpenCode, Aider, GitHub Copilot, Windsurf, Codex, Zed, and Trae
- **Anchor system** — automatically links AI sessions to git commits with code attribution data
- **Orphan branch storage** (`oobo/anchors/v1`) — metadata travels with the repository, no external dependencies
- **Local SQLite database** for session, project, and stats storage with automatic migrations
- **Token counting** via tiktoken with character-estimation fallback
- **AI code attribution** — detects which lines were written with AI assistance
- **Secret redaction** using gitleaks patterns and regex fallback before sharing
- **Interactive TUI** for dashboard, sessions, projects, and stats (`oobo dash`, `oobo sessions`, `oobo projects`, `oobo stats`)
- **Session sharing** (`oobo share`) with redaction and optional upload to configured endpoint
- **API usage tracking** for Anthropic and OpenAI accounts
- **Agent lifecycle hooks** for Cursor, Claude Code, Gemini CLI, and OpenCode
- **Per-project git hooks** (post-commit, pre-push) for automatic anchor creation
- **First-use setup wizard** with tool detection and configuration
- **Transparency modes**: Off (metadata only) and On (metadata + redacted transcripts)
- **Enterprise/cloud support**: authentication, remote event posting, self-hosted endpoints
- **Cross-platform install script** with platform detection and PATH management
- **CI/CD pipeline**: multi-platform testing (Ubuntu, macOS, Debian, Alpine) and 6-target release builds
- **Dual license**: Apache 2.0 and MIT

[0.1.0]: https://github.com/ooboai/oobo/releases/tag/v0.1.0
