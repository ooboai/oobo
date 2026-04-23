# `oobo hooks` (hidden)

Internal plumbing called by installed git hooks and AI-tool session hooks. NOT intended to be typed by users. Hidden from `oobo --help` output. Documented here because:

1. It's a stable surface — installed hook scripts depend on these invocation shapes.
2. Third-party tool adapters that want to integrate with oobo call this.
3. Debugging issues often means running one of these manually.

All `hooks` subcommands:
- Exit `0` on success.
- Print nothing on stdout by default (pure side effects).
- Print warnings on stderr with prefix `oobo: warning: ...`; never fatal-error to avoid breaking user git/tool workflows.
- Write to `~/.oobo/logs/hooks-debug.log` for diagnosability (appends only; bounded rotation in future).
- Respect `OOBO_INTERCEPTED=1` to prevent re-entry when oobo calls git internally.

---

## `oobo hooks agent <event>`

Called by AI-tool session hooks (Cursor's `agent-start`/`agent-stop`, Claude Code's `session-start`/`stop` hooks, etc.) when a session begins or ends. The tool passes a JSON payload on stdin describing the session.

### Signature
`oobo hooks agent <event> [--tool <name>]`

- `<event>` — one of: `session-start`, `session-end`, `stop`. Unknown events are logged and ignored (exit `0` — never break the caller).
- `--tool <name>` — the tool firing the hook: `cursor`, `claude`, `gemini`, `codex`, `aider`, `copilot`, `zed`, `continue`, `opencode`, `factory-droid`. Case-insensitive.
- **stdin** — a JSON payload. Required shape:

```json
{
  "session_id": "string (required)",
  "tool": "string (optional — overridden by --tool if present)",
  "started_at": "ISO-8601 timestamp (optional; defaults to now for session-start)",
  "ended_at":   "ISO-8601 timestamp (optional; defaults to now for session-end)",
  "workspace":  "absolute path (optional; defaults to $CWD)",
  "meta":       { /* tool-specific metadata, stored verbatim */ }
}
```

Empty stdin is treated as `{}` (payload-free hook, reasonable for simple `stop` events).

### Invocation: `session-start`

`oobo hooks agent session-start --tool cursor <<< '{"session_id":"abc123","workspace":"/Users/teddy/dev/oobo-cli"}'`

**Behavior:**
1. Resolve the project (by `workspace` path → `project_id` via the usual `remote_url` / `initial_commit_sha` / `primary_path` lookup).
2. Insert/UPSERT a row into `active_sessions` table with `session_id`, `tool`, `project_id`, `started_at` = payload.started_at or now().
3. Log the event to `~/.oobo/logs/hooks-debug.log`.

**Side effects:**
- DB row in `active_sessions`.
- Log line appended.

**Exit code:** `0`.

### Invocation: `session-end` / `stop`

`oobo hooks agent stop --tool claude <<< '{"session_id":"abc123"}'`

**Behavior:**
1. Look up the `active_sessions` row by `(session_id, tool)`.
2. Update its `ended_at` to payload.ended_at or now().
3. Eagerly enrich the session: parse transcript, compute tokens, extract intent (for search indexing).
4. Move it from `active_sessions` to the main `sessions` table.
5. Link the session to any anchor (commit) that was created while it was active (time-window match — refined by transcript cross-reference in later phases).
6. Log.

**Side effects:**
- DB: delete from `active_sessions`, insert/UPDATE into `sessions`, possibly insert into `session_anchors` link table.
- Background transcript parse (not blocking the caller).

**Exit code:** `0`.

### Invocation: unknown event

`oobo hooks agent fart --tool cursor <<< '{}'`

**Behavior:** Log and return `0`. NEVER fail the caller, who is the user's AI tool.

**stderr (optional warning):**
```
oobo: warning: unknown agent event 'fart' (tool=cursor). ignored.
```

**Exit code:** `0`.

### Invocation: malformed JSON on stdin

`echo 'not json' | oobo hooks agent session-start --tool cursor`

**Behavior:** Log + warn; treat as `{}`. Exit `0`.

**stderr:**
```
oobo: warning: could not parse agent payload as JSON. using empty payload.
```

### Agent env / non-TTY

No difference — this command has no TTY-aware behavior. Always silent stdout.

---

## `oobo hooks post-commit`

Called by the `post-commit` git hook installed into each enabled repo.

### Signature
`oobo hooks post-commit [git-passed-args-ignored...]`

Git's post-commit hook receives no args today, but we accept trailing args and ignore them (forward-compat).

### Behavior
1. Early-exit if `OOBO_INTERCEPTED=1` is set (prevents re-entry when oobo's own commit interceptor calls git internally).
2. Locate the project root.
3. Resolve / insert the project row.
4. Call `crate::git::interceptor::on_write_op(&cfg, &["commit"])`:
   - Read `HEAD` SHA, commit metadata.
   - Find active sessions matching the time window.
   - Insert an anchor row and `session_anchors` link rows.
   - Spawn a detached thread to write the orphan-branch file (non-blocking).
5. Cleanup stale entries in `active_sessions` older than 86400 s (24 h).

**Side effects:**
- Anchor row inserted in DB.
- Orphan-branch file written (asynchronously).
- Stale `active_sessions` rows pruned.

**Exit code:** `0` regardless of internal errors. A warning on stderr is allowed; breaking the user's commit workflow is NEVER allowed.

### Disabled project

**Behavior:** Short-circuit at step 3 after the project lookup. No anchor created. Log one line to the debug file.

**Exit code:** `0`.

### Concurrent commits (rebase, cherry-pick)

**Behavior:** Tolerate: UPSERTs are idempotent; duplicate invocations for the same SHA succeed silently.

---

## `oobo hooks pre-push`

Called by the `pre-push` git hook installed in each enabled repo.

### Signature
`oobo hooks pre-push [git-passed-args-ignored...]`

Git passes `<remote> <url>` plus refs on stdin, but we don't need them — we always push our own orphan branch when it exists.

### Behavior
1. Locate project root.
2. If the orphan branch `oobo/anchors/v1` exists → push it to `origin` (or configured remote).
3. Retry any previously failed pushes (queue in DB).
4. Log outcome.

**Side effects:**
- Network I/O: one `git push origin oobo/anchors/v1`.
- DB: orphan-branch push queue drained or retried.

**Exit code:** `0`. Failure to push anchors MUST NOT block the user's primary push — log, queue for retry, move on.

### Offline / remote unreachable

**Behavior:** Queue the push in DB for retry on next `pre-push` invocation.

**stderr:**
```
oobo: warning: could not push anchors: network unreachable. queued for retry.
```

---

## `oobo hooks post-merge`  (new in v1.0)

Called by `post-merge` git hook. Fires after `git pull` / `git merge` completes.

### Signature
`oobo hooks post-merge [git-passed-args-ignored...]`

### Behavior
1. Fetch the remote's orphan branch (`git fetch origin oobo/anchors/v1:refs/remotes/origin/oobo/anchors/v1`).
2. Hydrate any new anchors from the fetched tip into the local DB.
3. Skip silently if no remote orphan branch exists.

**Side effects:**
- Network I/O: one fetch.
- DB: new rows in `anchors` (idempotent UPSERT).

**Exit code:** `0` regardless.

---

## `oobo hooks post-rewrite`  (new in v1.0)

Called by `post-rewrite` hook. Fires after `git commit --amend`, `git rebase`. Receives on stdin a list of `old-sha new-sha` pairs.

### Signature
`oobo hooks post-rewrite [rebase|amend]`

### Behavior
1. Read stdin, parse pairs.
2. For each pair, update the anchor's `sha` from `old-sha` to `new-sha` in the DB (and in the orphan branch — scheduled for the next push).
3. If the pair represents a deletion (rewrite drops a commit), mark the anchor as `orphaned` rather than deleting it — preserves the AI session memory even when the commit is gone.

**Side effects:**
- DB: anchor rows updated or marked orphaned.
- Orphan branch: file renames queued for next push.

**Exit code:** `0`.

---

## Debug + introspection

### Invocation: dump hook log
`tail -n 50 ~/.oobo/logs/hooks-debug.log`

Typical line format:

```
2026-04-22T14:03:12Z event=session-start tool=Some("cursor") payload={"session_id":"abc","workspace":"/Users/teddy/dev/oobo-cli"}
2026-04-22T14:04:08Z post-commit sha=a1b2c3d4 project_id=0f5c linked_sessions=1
2026-04-22T14:04:10Z pre-push pushed_orphan=oobo/anchors/v1 result=ok
```

### Manual hook replay (debugging)

`OOBO_INTERCEPTED=0 oobo hooks post-commit` inside a repo — replays the hook logic for the current HEAD. Useful for diagnosing missed anchors.

---

## Invariants

- Every `oobo hooks` subcommand exits `0` on success AND on non-fatal internal errors. Exit `!= 0` is reserved for hard bugs (e.g. unparseable clap args).
- None of the hook subcommands write to stdout by default. Stderr is limited to warnings.
- `OOBO_INTERCEPTED=1` short-circuits the post-commit body (prevents re-entry).
- The append-only log at `~/.oobo/logs/hooks-debug.log` is the canonical debug trail.
- Changing any of these subcommand signatures is a breaking change for installed hook scripts across users' machines — requires `oobo setup --repair` to refresh.
- `oobo hooks` with no subcommand → exit `2` with a clap error (but hidden from normal help).
