# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.6] - 2026-03-16

### Changed

- **Remote API endpoints** — added `/anchors/share` for session sharing (auth optional, anonymous shares supported). Renamed from `/api/v1/shares`.
- **Share output** — `oobo share --out` now produces markdown by default; use `.json` extension for JSON output
- **Default remote** — `api.oobo.ai` is now the documented default server (free accounts)
- **Self-hosting docs** — only `/anchors/ingest` is required; verify, health, and share are optional

### Added

- Markdown renderer for shared sessions (source, model, stats header + conversation)
- Extension-based output format detection in `oobo share --out` (`.json` → JSON, anything else → markdown)
- Comprehensive CHANGELOG entries for all releases v0.1.0–v0.1.5
- Self-hosting section in README with endpoint table and backend implementation guide
- Updated mintlify documentation

## [0.1.5] - 2026-03-15

### Changed

- **Remote API endpoints** — consolidated under `/anchors` namespace: `/anchors/ingest` (POST anchor data), `/anchors/verify` (GET auth check), `/anchors/health` (GET connectivity). Self-hosted servers implement only these three endpoints.

### Fixed

- **Critical: self-update rollback** — `fs::copy` failure after backup no longer leaves the user without a working binary (C-2)
- **Critical: OpenCode adapter** — `read_transcript_by_id` override fixes empty transcripts caused by hardcoded `None` in session ID extraction (C-1)
- Race conditions in orphan branch temp index files (PID+nanos scoped) and `git update-ref` (compare-and-swap guard) (H-1, H-2)
- Copilot JSONL, Windsurf SQLite, Cursor JWT extraction resilience — malformed lines/DB errors no longer abort entire sessions (H-3, H-4, H-5)
- Database transaction atomicity for project deletion, busy timeout for concurrent access (H-7, H-8)
- Redaction temp file collision within same process (H-10)
- Git log attribution restricted to `--first-parent` (H-9)
- CI environment misclassification as Assisted (M-1)
- Empty session list panic in `filter_by_recency` (M-2)
- Git environment variable pollution in child processes (M-3)
- Failed rebase cleanup during push retry (M-5)
- Claude Code Windows path handling in `path_to_slug` (M-8)
- Codex timestamp overflow for millisecond values (M-12)
- `--since` flag silently ignored with `--project`/`--tool` — now warns (M-13)
- Unicode-safe string truncation in project/session names (M-14)
- Atomic config writes via tmp+rename (M-17)
- HTTP 500 vs 409 response handling in sync (M-19)
- AppleScript injection prevention in desktop notifications (M-20)
- Home directory fallback chain: `OOBO_HOME` → `HOME` → `USERPROFILE` (M-21)
- Model family detection false positives (L-29)
- Hook upgrade prevention removed — `merge_json_file` now always updates (L-30)
- Agent hint subcommand detection (L-1), path stripping (L-2), first-use marker TOCTOU (L-3), AI percentage clamped to 100% (L-5), Zed file extension filter (L-13)

### Added

- 13 new tests for OpenCode adapter validation (437 total, up from 424)
- Performance indexes on `sessions.created_at`, `sessions.updated_at`, `events.timestamp`, `session_stats.source` (M-22)
- Self-hosting documentation in README

## [0.1.4] - 2026-03-14

### Added

- **Per-edit file tracking** — `PostToolUse(Write|Edit)` hooks for Claude Code and `afterFileEdit` for Cursor track exactly which files the agent edited, replacing dirty-worktree heuristics
- **Claude Code hook parity** — `UserPromptSubmit` for per-turn snapshots, `SubagentStop` for subagent lifecycle (was missing)
- `edited_files` accumulator in `ActiveSession` state; stop handler prefers precise list over worktree scan
- `merge_claude_hooks_file` with nested matcher-group upsert semantics

### Fixed

- Centralized `is_cursor_agent` into `core::tool` to eliminate scattered alias checks
- Atomic session writes in `hooks/state.rs` (tempfile + rename)
- Added `CLAUDECODE` / `CLAUDE_CODE_ENTRYPOINT` / `CODEX_SANDBOX` environment variable detection
- Copilot transcript parsing now handles JSONL via `replay_jsonl`
- Decoupled `AiderTool::all_sessions` from `cursor::get_project_root`
- `ToolRegistry::by_name` matches on `config_key` (cursor vs composer)
- Canonicalize paths in `make_relative` to handle symlinks

## [0.1.3] - 2026-03-12

### Fixed

- **Non-TTY setup panic** — `oobo setup` in agent/CI environments no longer panics on `ratatui::init`; uses `try_init` with default config fallback
- **Anchor token persistence** — `insert_anchor_session` now persists `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_creation_tokens`, `duration_secs`, `tool_calls`, and `is_subagent`
- **Push sync 422** — skip `send_event` on push (no anchor to send), preventing 422 from ingestion API
- `oobo inspect --fix` returns `Fail` (not `Warn`) for missing agent hooks so the fix function actually runs

### Changed

- Install script rewritten with tiered PATH strategy — symlink to `/usr/local/bin` (root) or `~/.local/bin` (user), auto-detect shell rc, auto-run `oobo setup`
- README expanded with all missing command docs (sessions export, sync, transparency, auth, ignore/unignore, projects, anchors, share, card, index)

## [0.1.2] - 2026-03-12

### Added

- **Developer card redesign** — white background with subtle border for better sharing, session-aware streak calculation, per-cell AI/human gradient split in heatmap
- Heatmap now reflects session-only days (previously invisible without git commits)

### Changed

- Removed Less/More gradient legend, simplified to AI + Human color key
- Increased spacing between heatmap grid and legend

## [0.1.1] - 2026-03-12

### Fixed

- **Double sync** — `OOBO_INTERCEPTED` env var set before `run_git` so post-commit hook skips redundant `on_write_op`
- **Offline resilience** — 2s connect timeout on sync HTTP client (was blocking full 10s on offline commits)
- **Live session stats** — extract token stats at commit time from tool data files (Cursor `state.vscdb`, Claude transcripts, Gemini session JSON) instead of requiring prior `oobo index`

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
- **Session sharing** (`oobo share`) with redaction and optional upload
- **API usage tracking** for Anthropic and OpenAI accounts
- **Agent lifecycle hooks** for Cursor, Claude Code, Gemini CLI, and OpenCode
- **Per-project git hooks** (post-commit, pre-push) for automatic anchor creation
- **First-use setup wizard** with tool detection and configuration
- **Transparency modes**: Off (metadata only) and On (metadata + redacted transcripts)
- **Remote sync**: authentication and anchor posting via `/anchors` API (self-hosted or `api.oobo.ai`)
- **Cross-platform install script** with platform detection and PATH management
- **CI/CD pipeline**: multi-platform testing (Ubuntu, macOS, Debian, Alpine) and 6-target release builds
- **Dual license**: Apache 2.0 and MIT

[0.1.6]: https://github.com/ooboai/oobo/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/ooboai/oobo/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/ooboai/oobo/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/ooboai/oobo/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/ooboai/oobo/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/ooboai/oobo/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/ooboai/oobo/releases/tag/v0.1.0
