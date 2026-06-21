# `oobo hooks` (hidden)

Internal plumbing called by installed git hooks and AI-tool session hooks. NOT intended to be typed by users. Hidden from `oobo --help` output. Documented here because:

1. It's a stable surface  --  installed hook scripts depend on these invocation shapes.
2. Third-party tool adapters that want to integrate with oobo call this.
3. Debugging issues often means running one of these manually.

All `hooks` subcommands:
- Exit `0` on success.
- Print nothing on stdout by default (pure side effects).
- Print warnings on stderr with prefix `oobo: warning: ...`; never fatal-error to avoid breaking user git/tool workflows.
- Write to `${XDG_DATA_HOME:-$HOME/.local/share}/oobo/logs/hooks.log` for diagnosability (appends only; bounded rotation in future).
- Respect `OOBO_INTERCEPTED=1` to prevent re-entry when oobo calls git internally.

---

## Installed hooks by editor

`oobo setup` installs agent lifecycle hooks for each supported tool:

| Tool            | Config path                                  | Hook format         |
|-----------------|----------------------------------------------|---------------------|
| Cursor          | `~/.cursor/hooks.json`                       | `{ "version": 1, "hooks": { "<event>": [{ "command": "..." }] } }` |
| Claude Code     | `~/.claude/settings.json`                    | Claude matcher-group format |
| Gemini CLI      | `~/.gemini/settings.json`                    | JSON merge          |
| OpenCode        | `~/.config/opencode/plugins/oobo.ts`         | TypeScript plugin   |
| Kiro            | `~/.kiro/agents/oobo.json`                   | Agent config JSON   |
| Continue        | `~/.continue/settings.json`                  | Claude-compatible   |
| Factory Droid   | `~/.factory/settings.json`                   | Claude-compatible   |

Per-repo git hooks are installed into `.git/hooks/`:
- `post-commit`  --  calls `oobo hooks post-commit`
- `pre-push`  --  calls `oobo hooks pre-push`
- `post-merge`  --  calls `oobo hooks post-merge`
- `post-rewrite`  --  calls `oobo hooks post-rewrite`

Existing user hooks are preserved: the original is backed up to `<hook>.pre-anchor` and chained.

---

## Session state persistence

Session state is stored as **JSON buffer files**, not SQLite.

- **Primary store:** `~/.oobo/tmp/hook-buffer/{session_id}.json`  --  one file per active session, written atomically via `tempfile` + rename.
- **Legacy fallback (read-only):** `.git/oobo-sessions/{session_id}.json`  --  files written by oobo 0.1.x. Read on lookup; never written to.
- **Read path:** buffer file → legacy file. First hit wins.
- **Write path:** always the buffer file.

Each file contains a serialized `ActiveSession` struct with fields including:
- `session_id`, `agent`, `model`, `worktree`
- `edited_files`, `read_files`, `tool_usage`, `tool_failures`, `bash_commands`
- `pre_agent_snapshots`, `file_snapshots` (git blob hashes)
- `pre_edit_pending`, `file_edit_chain` (per-edit attribution)
- `subagent_runs`, `thinking_duration_ms`, `compact_count`, `turn_count`
- `context_tokens`, `context_window_size`
- `current_turn_index`, `current_turn_started_at`, `current_turn_hook_events`, `current_turn_tool_calls`
- `started_at`, `updated_at`

Session IDs are sanitized: only ASCII alphanumeric, `-`, `_`, and `.` are allowed; other characters are mapped to `_`, and `..` sequences are collapsed to prevent path traversal.

---

## `oobo hooks agent <event>`

Called by AI-tool session hooks when lifecycle events fire. The tool passes a JSON payload on stdin.

### Signature
`oobo hooks agent <event> [--tool <name>]`

- `<event>`  --  lifecycle event name (see table below). Unknown events warn and return `0`.
- `--tool <name>`  --  the tool firing the hook: `cursor`, `claude`, `gemini`, `codex`, `aider`, `copilot`, `zed`, `continue`, `opencode`, `kiro`, `droid`. Case-insensitive. Overrides `agent` field in payload.
- **stdin**  --  a JSON payload:

```json
{
  "session_id": "string (required for most events)",
  "agent": "string (optional  --  overridden by --tool if present)",
  "model": "string (optional)",
  "cwd": "absolute path (optional)",
  "workspace_roots": ["path", ...],
  "loop_count": 42,
  "context_tokens": 12000,
  "context_window_size": 200000
}
```

Unknown fields are captured in an `extra` map for forward compatibility. Empty stdin is treated as `{}`.

### Handled events

| Event                  | Behavior |
|------------------------|----------|
| `session-start`        | Create `ActiveSession` in buffer file. Snapshot pre-agent file state for non-Cursor agents. |
| `session-end`          | Remove the session's buffer file. |
| `before-submit-prompt` | Ensure session exists, start a new turn, snapshot pre-agent dirty files via `git hash-object -w`. |
| `pre-tool-use`         | For file-mutating tools, snapshot the file's git blob hash before the edit (`pre_edit_pending`). |
| `after-tool-use` / `after-file-edit` | Record tool usage counts, tool call details, edited/read files. Pair with pre-edit blob to build the edit chain. |
| `tool-use-failure`     | Record failed tool call. Increments `tool_failures` and `tool_usage` count. |
| `subagent-start`       | Record subagent spawn (agent_id, agent_type, timestamp). |
| `subagent-stop`        | Set `ended_at` on the subagent. Snapshot modified files. Finish the current turn. |
| `after-agent-thought`  | Accumulate `thinking_duration_ms` from `duration_ms` in payload. |
| `after-agent-response` | Touch session timestamp. |
| `pre-compact`          | Increment `compact_count`. |
| `stop`                 | Update session metrics (loop_count, context_tokens). Snapshot edited files. Finish the current turn (writes a git-backed turn snapshot). |
| (unknown)              | Log warning, return `0`. |

### Invocation: `session-start`

`oobo hooks agent session-start --tool cursor <<< '{"session_id":"abc123"}'`

**Behavior:**
1. Resolve project root from `workspace_roots` / `cwd` / `$CWD`.
2. Create an `ActiveSession` and write it to `~/.oobo/tmp/hook-buffer/abc123.json`.
3. For non-Cursor agents, snapshot pre-agent dirty files.

**Side effects:** Buffer file created. Log line appended.

**Exit code:** `0`.

### Invocation: `stop`

`oobo hooks agent stop --tool claude <<< '{"session_id":"abc123"}'`

**Behavior:**
1. Ensure the session exists in the buffer.
2. Update session metrics from the payload (`loop_count`, `context_tokens`, `context_window_size`).
3. Snapshot edited files into git's object store (`git hash-object -w`).
4. Finish the current turn  --  write a `TurnSnapshot` to a git ref under the orphan branch.
5. Touch session timestamp.

**Side effects:** Buffer file updated. Turn snapshot written to git refs. Log line appended.

**Exit code:** `0`.

### Invocation: `session-end`

`oobo hooks agent session-end --tool cursor <<< '{"session_id":"abc123"}'`

**Behavior:** Remove the buffer file at `~/.oobo/tmp/hook-buffer/abc123.json`. Also removes any legacy file.

**Exit code:** `0`.

### Invocation: unknown event

`oobo hooks agent fart --tool cursor <<< '{}'`

**Behavior:** Log and return `0`. NEVER fail the caller, who is the user's AI tool.

**Exit code:** `0`.

### Invocation: malformed JSON on stdin

`echo 'not json' | oobo hooks agent session-start --tool cursor`

**Behavior:** Log + warn; skip the event. Exit `0`.

### Agent env / non-TTY

No difference  --  this command has no TTY-aware behavior. Always silent stdout.

---

## Pre/post edit chain mechanism

The `preToolUse` → `postToolUse` hook pair captures per-edit file attribution:

1. **`pre-tool-use`**: For file-mutating tools (`Write`, `Edit`, `StrReplace`, `Delete`, etc.), run `git hash-object -w` on the file to get the pre-edit blob hash. Store it in `pre_edit_pending[rel_path]`.
2. **`after-tool-use`**: Run `git hash-object -w` again to get the post-edit blob hash. If a pre-edit hash exists for this file and the hashes differ, create a `FileEditPair { pre_blob, post_blob, tool_name, timestamp }` and append it to `file_edit_chain[rel_path]`. If the hashes are identical (no-op edit), no pair is created.
3. Multiple edits to the same file within a turn produce a chain where each pair's `post_blob` equals the next pair's `pre_blob`.
4. The edit chain is reset at the end of each turn (`finish_turn`).

For new files, `pre_blob` is set to the null hash (`0000000000000000000000000000000000000000`).

---

## Turn snapshot mechanism

Turns are delimited by `before-submit-prompt` (start) and `stop` / `subagent-stop` (end). Each completed turn produces a `TurnSnapshot` written as a git ref on the orphan branch.

A turn snapshot contains:
- Project ID, worktree ID, source tool, session ID, turn index
- Parent snapshot ID (linked list of turns within a session)
- Started/ended timestamps
- Per-file pre/post blob hashes (from the edit chain when available, otherwise from session-level snapshots)
- Memory payload: transcript path, hook events, tool calls

Turn index increments after each snapshot. The `last_turn_snapshot_id` on the session links turns into a chain.

---

## `oobo hooks post-commit`

Called by the `post-commit` git hook installed into each enabled repo.

### Signature
`oobo hooks post-commit [git-passed-args-ignored...]`

### Behavior
1. Early-exit if `OOBO_INTERCEPTED=1` is set.
2. Locate the project root.
3. Call `on_write_op` which appends the commit SHA to the spool file and kicks an async worker to do the heavy enrichment (session matching, anchor creation) in the background.
4. Cleanup stale session buffer files older than 86400 s (24 h).

**Side effects:** Commit appended to spool. Async worker spawned to create anchor on orphan branch. Stale buffer files pruned.

**Exit code:** `0` regardless of internal errors.

### Disabled project

**Behavior:** Short-circuit after project lookup. No anchor created.

**Exit code:** `0`.

### Concurrent commits (rebase, cherry-pick)

**Behavior:** Tolerate: duplicate invocations for the same SHA succeed silently.

---

## `oobo hooks pre-push`

Called by the `pre-push` git hook installed in each enabled repo.

### Signature
`oobo hooks pre-push [git-passed-args-ignored...]`

### Behavior
1. Locate project root.
2. If the orphan branch `oobo/anchors/v2` exists → push it to `origin` (or configured remote). A legacy `oobo/anchors/v1` branch (read-only, pre-v2 data) is pushed too when present.
3. Log outcome.

**Side effects:** Network I/O: one `git push origin oobo/anchors/v2` (plus the legacy v1 branch when it exists).

**Exit code:** `0`. Failure to push anchors MUST NOT block the user's primary push.

---

## `oobo hooks post-merge`

Called by `post-merge` git hook after `git pull` / `git merge` completes.

### Signature
`oobo hooks post-merge [git-passed-args-ignored...]`

### Behavior
Fetch the remote orphan branch when needed and hydrate new anchors locally.

**Exit code:** `0`. Hydration failures warn at most and never break the user's merge/pull workflow.

---

## `oobo hooks post-rewrite`

Called by `post-rewrite` hook after `git commit --amend` or `git rebase`.

### Signature
`oobo hooks post-rewrite [git-passed-args-ignored...]`

### Behavior
Best-effort remap of anchor SHAs from old to new commits, preserving AI session memory across history rewrites.

**Exit code:** `0`. Rewrite remapping failures warn at most and never break the user's amend/rebase workflow.

---

## Session cleanup

Stale sessions are cleaned up opportunistically during `post-commit`:

1. List all sessions for the project (buffer files + legacy files).
2. Remove any whose `updated_at` is older than 86400 s (24 h).
3. Separately sweep the buffer directory (`~/.oobo/tmp/hook-buffer/`) for files older than the same threshold.

The `session-end` event removes the buffer file immediately. Cleanup is a safety net for sessions that never receive an explicit end event.

---

## Invariants

- Every `oobo hooks` subcommand exits `0` on success AND on non-fatal internal errors. Exit `!= 0` is reserved for hard bugs (e.g. unparseable clap args).
- None of the hook subcommands write to stdout by default. Stderr is limited to warnings.
- `OOBO_INTERCEPTED=1` short-circuits the post-commit body (prevents re-entry).
- Session state is persisted as JSON buffer files in `~/.oobo/tmp/hook-buffer/`, not in SQLite.
- The append-only log at `${XDG_DATA_HOME:-$HOME/.local/share}/oobo/logs/hooks.log` is the canonical debug trail.
- Changing any of these subcommand signatures is a breaking change for installed hook scripts across users' machines  --  requires `oobo setup --repair` to refresh.
- `oobo hooks` with no subcommand → exit `2` with a clap error (but hidden from normal help).
