# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0-rc.1] - 2026-04-22

Major sharpening release. Significantly narrower command surface, unified
output modes, and a flagship `anchors` experience.

### Added

- **`oobo anchors show <sha>`** drill-down (pretty / agent / json). Accepts unambiguous SHA prefixes.
- **`oobo anchors` filters**: `--since <24h|7d|ISO>`, `--tool`, `--project` (outside-repo only).
- **`oobo blame`** is now a strict superset of `git blame`. Every `git blame` flag is forwarded; an AI-attribution column is overlaid for pretty/agent mode. `--no-ai`, `--porcelain`, `--line-porcelain`, `--incremental` bypass the overlay.
- **`oobo search`** promoted to a top-level verb. Local DB search with `--since`, `--project`, `--tool`, `--limit`.
- **`oobo settings`** declarative KV config with positional grammar: `oobo settings [scope] [verb] <key> [value]`. Scope defaults to `default`, verb defaults to `get`. Keys: `key`, `remote`, `transparency`, `tools.experimental`, `setup.scan_roots`.
- **`oobo enable` / `oobo disable`** per-project tracking toggles (idempotent).
- **`oobo alias install` / `oobo alias uninstall`** manage the `alias git=oobo` shell alias.
- **`oobo setup`** rebuilt as a composable entry point: `--non-interactive`, `--reindex`, `--uninstall-alias`, `--repair`.
- **Auto-indexing**: view commands (`anchors`, `blame`, `search`, bare `oobo`) kick a fire-and-forget rescan when `last_scanned_at` is older than 5 minutes. Opt out with `OOBO_NO_AUTO_INDEX=1`.
- **Legacy command hint system**: removed 0.1.x verbs now print a hint and, in a TTY, offer to run the replacement. Scheduled for removal in 1.1.
- **Agent auto-detection**: `--agent` auto-activates when stdout is not a TTY or any of `CURSOR_AGENT`, `CLAUDECODE`, `AIDER`, `CONTINUE_SESSION`, `CONTINUE_IDE`, `AICOMMITS` is set.
- **Four-quadrant bare `oobo`** behavior (in-repo/outside × pretty/structured).
- **Stable project-identity helpers** (`src/project.rs`): canonicalizes git remote URLs so SSH and HTTPS forms collapse to the same key.
- **`tests/cli-spec/`** — behavioral contract for every 1.0 command.

### Changed

- **Output modes** are three mutually exclusive modes: pretty (TTY), agent (`--agent`, token-efficient plain text for LLMs), json (`--json`).
- **`oobo anchors --json`** now emits a flat JSON array (no envelope).
- **Default anchors limit** raised from 10 → 50.
- Help output regrouped into Anchor / Recall / Settings / Lifecycle / Git passthrough sections.

### Removed

- `scan`, `sessions`, `projects`, `ignore`, `unignore`, `sync`, `auth`, `transparency`, `card`, `dash`, `sources`, `inspect`, `stats`, `share`, `export`, `version`, `doctor`, `agent`, `index`. Each has a legacy hint mapping to its replacement.
- Vendor billing pipeline (internal API routes + usage tracking).

### Migration notes (0.1.x → 1.0.0)

- `oobo scan` → `oobo setup --reindex` (or just wait; view commands auto-index).
- `oobo ignore` / `oobo unignore` → `oobo disable` / `oobo enable`.
- `oobo sync` / `oobo auth` → `oobo settings set key <...>`.
- `oobo share`, `oobo export` → `oobo anchors show <sha> --json`.
- `oobo sessions` → inside `oobo anchors show <sha>` or `oobo search`.

## [0.1.15] - 2026-04-08

### Added

- **5 new agent integrations** — Amp, Continue, Factory Droid, Junie, and Kiro are now supported with full session discovery, transcript parsing, and message indexing. Brings total supported tools from 10 to 15.
- **Windows installer** — new `install.ps1` PowerShell script for native Windows installation alongside the existing bash script.
- **Homebrew CI** — automated Homebrew formula testing in the CI pipeline.

### Fixed

- **`inspect --fix` now repairs warnings** — previously only errors triggered the fix function; warnings were silently skipped. Stale session data is also preserved across fix runs.
- **Card terminal rendering** — corrected month label alignment, heatmap recency ordering, model bar layout proportions, and streak calculation cap.
- **Attribution pipeline** — fixed anchor column lookup, widened session time windows for matching, deduplicated `ai_commits` entries, and added processing for orphaned anchors that previously went unlinked.
- Resolved clippy lints (`char` comparison, `match_result_ok`) and `cargo fmt` formatting for CI.

## [0.1.14] - 2026-03-27

### Added

- **Per-line AI/human code attribution** — every commit anchor now records which specific line ranges were written by AI vs. human. Uses a three-point diff (pre-agent → agent snapshot → committed) to compute precise line-level attribution in committed-file coordinates. Data is stored in `line_attributions` on each `FileChange` entry in the anchor JSON.
- **`oobo blame` command** — new command that displays per-line AI attribution for any file at any commit. Shows colorized output with line numbers, author labels (ai/human), and agent names. Supports `--json` and `--agent` output modes. File path input is normalized (handles `./`, absolute paths) for reliable matching.
- **Post-rewrite anchor re-keying** — when `git rebase` or `git cherry-pick` rewrites commit history, oobo now automatically copies all anchor data (metadata, session links, transcripts, timeline) from old SHAs to new SHAs by matching tree hashes. Preserves attribution through simple rebases where file content doesn't change.
- **Modular skill file** — `SKILL.md` restructured into a concise quick-reference with detailed reference docs split into `references/COMMANDS.md`, `references/API_SURFACE.md`, `references/JSON_SCHEMA.md`, and `references/TRUST.md`. Reduces token cost for agents that only need the command table.

### Fixed

- **Developer card: AI percentage showed 0% with no commit data** — `ai_percentage` now returns `None` (displayed as "N/A") when there are zero tracked commits, instead of a misleading "0% AI".
- **Developer card: AI streak over-counted past idle gaps** — `compute_ai_streak` no longer counts disconnected old AI days that appear after a tail of idle days. Idle days at the end correctly break the streak.
- **Developer card: SVG stat text misaligned with Unicode** — stat label/value width calculation now uses per-character Unicode width instead of byte length, fixing layout for CJK characters and emoji.
- **Developer card: earliest session date in wrong timezone** — `earliest_session` now formats dates in the user's local timezone instead of UTC.
- **Developer card: `--json` included SVG unconditionally** — `oobo card --json` no longer embeds the full SVG string unless writing to a file, reducing payload size for programmatic consumers.
- **Multi-session snapshot collision silent** — when multiple AI sessions provide file snapshots for the same file, oobo now logs a warning to stderr instead of silently using the last writer's data.

## [0.1.13] - 2026-03-25

### Fixed

- **Session stats never refreshed for active sessions** — once a session was indexed, its token counts, duration, and file stats were never recomputed even if the user kept chatting. All code paths (`oobo sessions`, `oobo index`, `oobo scan`) now detect stale stats by comparing `updated_at` against `computed_at` and re-index automatically.
- **TUI and inline indexers only processed sessions with no stats** — sessions with outdated stats were silently skipped. Both the inline indexer (agent/JSON mode, capped at 20) and the TUI background indexer (capped at 100) now also re-index stale sessions.
- **Stale stats reload discarded fresh data** — after re-indexing, `stats_map.entry().or_insert()` would keep the old entry; changed to `insert()` so refreshed stats actually replace stale ones.

### Changed

- **`StatsRow::is_stale()` method** — staleness logic moved from a standalone function in `commands::sessions` to a method on `StatsRow`, reducing coupling and making it testable without a full `Session` struct.
- **Shared SQL timestamp normalization** — the `CASE WHEN` expression that normalizes `updated_at` (seconds/millis/micros) to epoch seconds is now a constant (`UPDATED_AT_EPOCH_SECS_SQL`) shared between both DB queries, preventing drift with the Rust-side `to_epoch_secs()`.
- **Targeted stats reload** — after inline re-indexing, only the affected sessions are reloaded via individual `get_stats()` calls instead of fetching the entire `session_stats` table.

## [0.1.12] - 2026-03-23

### Fixed

- **Privacy: absolute path leakage in public metadata** — `bash_commands` on the orphan branch and remote payload contained raw absolute paths (e.g. `/Users/teddy/dev/projects/...`), leaking usernames and directory structures on public repos. All publicly-visible fields now run through `sanitize_for_public()` which strips project root paths, replaces home directory with `~/`, and redacts secrets via gitleaks.
- **`files_touched` absolute path leakage** — `file_snapshots` keys from subagent `modified_files` could carry absolute paths into `SessionLink.files_touched`. Now sanitized with `sanitize_path()` before serialization.
- **`file_interactions` absolute path leakage** — multi-agent file interaction paths are now sanitized before writing to the orphan branch or remote payload.
- **Subagent `modified_files` not relativized** — `subagent-stop` hook events pushed file paths directly into `snapshot_session_files` without `make_relative()`, unlike `after-tool-use` which already did. Now consistent.
- **Shared session path leakage** — `oobo share` now uses `sanitize_for_public()` (redact + strip paths) instead of just `redact()` on message text, preventing absolute paths from appearing in uploaded shared sessions.
- **Token estimation missing on anchors** — `SessionLink` objects sent to the backend had `input_tokens: null` / `output_tokens: null` whenever native token counts weren't available (common for Cursor sessions). Anchors now include tiktoken-estimated token counts as a fallback, marked `is_estimated: true`. Estimates use the correct BPE encoding per model family (cl100k for Claude, o200k for GPT-4o/o1/o3).
- **Token estimation undercounting by 20–60%** — `count_input_tokens` only counted `user` role messages, missing `system` prompts, `tool` results, and `function` outputs. All input-side roles are now counted.
- **Inconsistent native token predicate** — `compute_session_stats` treated `Some(0)` as native data while the interceptor treated it as missing. Both paths now use `> 0` to decide whether native tokens are present, falling through to tiktoken when the tool reported zero.
- **BPE tokenizer re-allocated per message** — `cl100k_base()` / `o200k_base()` were called per `count_tokens` invocation, allocating a new `CoreBPE` each time. Now cached in `OnceLock` statics — initialized once per process.
- **Redundant state.vscdb reads at commit time** — Cursor sessions triggered up to 3 SQLite reads of the same row (bubble data, composerData for duration, composerData for messages). Composer data is now preloaded once alongside bubble data and passed through to all consumers.

### Changed

- **`strip_absolute_paths` consolidated** — moved from a private function in `orphan.rs` to a shared public function in `redact/mod.rs`, alongside new `sanitize_for_public()` and `sanitize_path()` helpers. All public-facing serialization paths use the shared implementation.

## [0.1.11] - 2026-03-22

### Added

- **Compact `--agent` output mode** — `--agent` now produces compact, pipe-delimited text instead of JSON. Lists print a schema header (`# field | field | ...`) then one record per line. Single-object commands print `key: value` pairs. Designed for minimal token cost when agents read oobo output.
- **`--json` flag** — new global flag for full structured JSON output, replacing the previous `--agent` JSON behavior. Use `--json` for scripts or when the full object graph (messages, file attribution) is needed.
- **Multi-tool skill discovery** — `oobo setup` and `oobo update` now install the `SKILL.md` symlink in `~/.agents/skills/oobo/`, `~/.claude/skills/oobo/`, `~/.codex/skills/oobo/`, `~/.cursor/skills/oobo/`, and `~/.gemini/skills/oobo/`, ensuring all major AI coding tools already installed on the system discover the skill automatically.
- **Post-update migrations** — `oobo update` now runs post-update tasks automatically after installing a new version: refreshes the skill file, applies pending DB migrations, and re-installs agent hooks.
- **Subagent session hierarchy tracking** — parent-child relationships between agent sessions and their spawned subagents are now detected and displayed. Cursor (`subagentInfo`), Claude Code (`subagents/agent-*.jsonl`), Gemini CLI (`parentSessionId`), and OpenCode (`parent_id`) are all supported. Subagent sessions appear nested under their parent in the TUI with `└─ [type]` prefix, and `parent_session_id`/`subagent_type` fields are included in JSON output. Subagent transcripts are written to the orphan branch under `subagents/` within the parent session directory.
- **Multi-agent file interaction tracking** — when multiple AI sessions touch the same files in a project, oobo now detects and records these interactions. `read_files` are tracked via `after-tool-use` hooks for Read, Grep, Search, Glob, and similar tools. At commit time, `file_interactions` (with per-session Writer/Reader/Both roles) and `peer_session_ids` are computed and stored on anchors. The TUI shows interaction hints inline, and `peer_session_ids` appears in all `--json` output paths (list, show, search). A `timeline.json` file is generated on the orphan branch for commits with file interactions.
- **`detect_interactions()` shared algorithm** — centralized file interaction detection in `core/anchor.rs`, used by the interceptor (commit time), `oobo sessions` CLI (display time), and the TUI, eliminating DRY violations
- **`get_file_sets()`** — single-read function that returns both `edited_files` and `read_files` from session state, halving I/O for interaction detection
- **DB migration v8** — added `parent_session_id` and `subagent_type` columns to the `anchor_sessions` table

### Fixed

- Bumped `rustls-webpki` 0.103.9 → 0.103.10 to resolve RUSTSEC-2026-0049 (faulty CRL distribution point matching)

### Changed

- **Breaking: `--agent` output format** — `--agent` no longer produces JSON. Use `--json` for JSON output. This affects all commands that previously returned JSON via `--agent`. Notably, `sessions show <id> --agent` no longer includes the `messages` array — use `sessions show <id> --json` for the full conversation transcript.

## [0.1.10] - 2026-03-20

### Fixed

- **Multi-writer anchor branch race conditions** — replaced force-fetch reconciliation with PID-namespaced temp refs and CAS (compare-and-swap) `update-ref` operations, eliminating data loss when multiple users or agents push concurrently
- **Atomic replay in `replay_local_files`** — local-only files are now committed on top of the remote state *before* moving the branch ref, so a failure mid-replay never leaves the branch in a partially-updated state
- **FETCH_HEAD race** — concurrent fetches (IDE background refresh, other hooks) can no longer corrupt the reconciliation by overwriting the shared `FETCH_HEAD` file
- **Temp index file leak** — `build_commit_on` now cleans up its temporary index file on all code paths, including mid-pipeline errors
- **Push error message clobbering** — reconciliation errors are now chained with the original push failure instead of overwriting it
- **First-use branch creation TOCTOU** — `ensure_branch` and `reconcile_local_with` now use null-OID CAS to prevent concurrent branch creation from silently overwriting data

### Changed

- Renamed `orphan::fetch()` to `orphan::fetch_and_reconcile()` to accurately reflect its side effects
- Removed duplicate `fetch_remote_branch` from `sync.rs` in favor of centralized `orphan::fetch_and_reconcile`
- Improved jitter quality for retry backoff by mixing in the process ID to decorrelate concurrent retries

## [0.1.9] - 2026-03-18

### Added

- **Proactive session indexing** — session stats (tokens, model, duration) are computed automatically at session-end and commit time, eliminating the need to run `oobo scan` before `oobo sessions` shows data
- **Background indexing in TUI** — `oobo sessions` shows the table immediately with `...` placeholders for unindexed sessions, filling them in the background as stats are computed
- **Inline indexing for JSON/search** — `oobo sessions --json` and `oobo sessions search` index up to 20 unindexed sessions inline so output is complete
- **Model enrichment from hook state** — session model info recorded by hooks is now used during scanning and indexing, fixing the "model missing" issue for Cursor sessions
- **`normalize_source()` helper** — centralized agent-to-source mapping in `core/tool.rs`, replacing duplicated inline conditionals
- **`read_session()` / `read_session_model()`** — new functions in `hooks/state.rs` for reading session state files
- 11 new unit tests covering `read_session`, `read_session_model`, `merge_native_stats`, state enrichment, and inline indexing

### Fixed

- **`upsert_session` NULL clobber** — `name` and `mode` columns now use `COALESCE` in the upsert SQL, preventing proactive indexing from overwriting existing values with NULL
- **TUI "indexing..." indicator stuck forever** — replaced count-based progress tracking with `TryRecvError::Disconnected` detection so the indicator disappears when the background thread completes
- **Commit-time indexing latency** — session indexing at commit time now runs on a detached background thread instead of blocking the git commit path
- **Double Cursor data loads** — `index_single_session` now loads bubble/composer data once via `load_cursor_messages_and_enrich()` instead of loading it separately for messages and native stats
- **Redundant model file reads** — `index_sessions_inner` now checks `row.model` before falling back to reading the hook state file, avoiding double I/O during the scan → index pipeline

### Changed

- **Proactive indexing errors are now logged** — `eprintln!("oobo: warning: ...")` at session-end, commit-time, and inline indexing call sites (TUI background thread stays silent to avoid corrupting the terminal)

## [0.1.8] - 2026-03-17

### Added

- **Rich hook telemetry** — expanded from 4 hook events to 11 for Cursor and 8 for Claude Code. New events: `after-tool-use` (replaces `after-file-edit`), `tool-use-failure`, `subagent-start`, `after-agent-thought`, `after-agent-response`, `pre-compact`
- **Session state tracking** — 6 new fields on `ActiveSession`: `tool_usage` (per-tool call counts), `tool_failures`, `bash_commands` (capped at 50), `subagent_runs`, `thinking_duration_ms`, `compact_count`
- **Structured transcript parsing** — Claude JSONL transcripts now parse into rich `TranscriptMessage` with `thinking`, `tool_call`, `tool_result`, and `timestamp_ms` fields alongside text
- **SessionLink enrichment** — anchors now carry `tool_usage`, `tool_failures`, `subagent_count`, `bash_commands`, `thinking_duration_ms`, `compact_count`
- **Shared utilities** — `truncate_str()` (UTF-8 safe) and `summarize_tool_input()` in `utils.rs`, used by hooks, transcript parser, and interceptor
- 10 new unit tests covering state mutations, JSONL transcript parsing, unicode truncation, and message ordering

### Fixed

- **Unicode truncation panic** — all string truncation now uses char-boundary-safe `truncate_str()` instead of byte-offset slicing
- **Tool results missing from transcripts** — `tool_result` blocks are now correctly parsed from user entries in Claude JSONL (where Claude actually places them)
- **ToolResultMessage.name always empty** — parser now maintains a `tool_use_id → name` map to populate the name field
- **Transcript message ordering** — messages now emit in correct order: thinking → text → tool_calls (was tool_calls first)
- **bash_commands not redacted** — commands in `SessionLink` now run through `redact()` before backend payload
- **Stale doc comment** — `record_edited_file` now references `after-tool-use` instead of removed `after-file-edit`

### Changed

- **`TranscriptMessage.text`** changed from `String` to `Option<String>` — messages with only thinking/tool_call/tool_result omit the text field. Backends using `serde(default)` handle this transparently.
- **Claude `PostToolUse` matcher removed** — hook now fires for all tool types, not just Write/Edit
- **Deduplicated transcript parsing** — single canonical `parse_rich_transcript_lines()` replaces ~200 lines of duplicated code across `transcript.rs` and `interceptor.rs`
- **`summarize_tool_input`** consolidated from 3 divergent implementations into one shared function with Cursor-specific tool name support (`StrReplace`, `codebase_search`, `SemanticSearch`, etc.)

## [0.1.7] - 2026-03-16

### Fixed

- **Critical: Cursor session tracking** — removed erroneous `alias = "conversation_id"` on the `cwd` serde field in `HookEvent`. This alias silently consumed Cursor's `conversation_id` (used as session identifier), making it invisible to the session ID resolver. All `ensure_session` and session file creation calls were skipped, causing no session files to be created in `.git/oobo-sessions/` for Cursor sessions. Other tools (Claude Code, Gemini CLI, etc.) that send `session_id` directly were unaffected.

### Added

- Debug logging in `ensure_session` for diagnosing session file creation failures

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

[0.1.15]: https://github.com/ooboai/oobo/compare/v0.1.14...v0.1.15
[0.1.14]: https://github.com/ooboai/oobo/compare/v0.1.13...v0.1.14
[0.1.13]: https://github.com/ooboai/oobo/compare/v0.1.12...v0.1.13
[0.1.12]: https://github.com/ooboai/oobo/compare/v0.1.11...v0.1.12
[0.1.11]: https://github.com/ooboai/oobo/compare/v0.1.10...v0.1.11
[0.1.10]: https://github.com/ooboai/oobo/compare/v0.1.9...v0.1.10
[0.1.9]: https://github.com/ooboai/oobo/compare/v0.1.8...v0.1.9
[0.1.8]: https://github.com/ooboai/oobo/compare/v0.1.7...v0.1.8
[0.1.7]: https://github.com/ooboai/oobo/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/ooboai/oobo/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/ooboai/oobo/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/ooboai/oobo/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/ooboai/oobo/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/ooboai/oobo/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/ooboai/oobo/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/ooboai/oobo/releases/tag/v0.1.0
