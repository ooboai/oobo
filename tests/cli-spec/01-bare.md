# Bare `oobo` — no subcommand

Bare `oobo` is the primary feed command. It resolves along two dimensions: (1) inside a git repo or not, (2) output mode (pretty / agent / json). Outside a repo it always shows a hint.

Filter flags (`-n`, `--since`, `--tool`) are global and apply to bare `oobo` directly.

---

## Inside a git repo, pretty mode (TTY, no flags, no agent env)

### Invocation
`oobo`

**Context:** `$CWD` is inside a git repo that is `enabled` in oobo. TTY attached. No agent env vars.

**Behavior:** Open the anchor-feed TUI. Keys: `↑` / `↓` navigate, `Enter` drills into `oobo anchor show <sha>` view, `/` opens search, `q` quits. Header shows aggregate stats (anchors, tokens, AI%). If the project is disabled, show the disabled-banner variant (see below).

**Example output (screen buffer, shape):**
```
┌─ oobo · myrepo ───────────── 847 anchors · 1.2M tok · 42% AI ─────┐
│                                                                   │
│  ● 2m    fix auth middleware               claude · 12k · 1 sess  │
│  ● 18m   add rate limiter                  gemini · 31k · 2 sess  │
│  ● 1h    wip                               (local only, no sync)  │
│                                                                   │
└─ ↑↓ nav · enter open · / search · q quit ────────────────────────┘
```

**Exit code:** `0` on clean `q` quit.

**Side effects:** may trigger a background index refresh in a detached thread (no blocking, no output).

### Disabled-project variant

**Behavior:** Same TUI shell, but the header shows `· DISABLED ·` in place of stats, and the body shows a one-screen explanation + hint: "Run `oobo enable` to start tracking, or `oobo setup` to pick which projects to track."

---

## Inside a git repo, agent mode

### Invocation
`oobo --agent`

**Context:** inside a git repo, enabled.

**Behavior:** Minimal one-line-per-anchor listing (default limit 50). See `02-anchors.md` for full column spec.

**Example output:**
```
a1b2c3d 2m   fix auth middleware        claude 12k 1s
d4e5f6g 18m  add rate limiter           gemini 31k 2s
7a8b9c0 1h   wip                        -      -   -
e1f2d3c 3h   extract payment adapter    cursor 28k 1s
```

**Exit code:** `0`.

**Side effects:** none.

---

## Inside a git repo, JSON mode

### Invocation
`oobo --json`

**Behavior:** Full structured anchor list for this repo (default limit 50).

**Example output:**
```json
{
  "project": { "id": "0f5c...", "path": "$REPO", "remote": "git@github.com:me/repo.git", "enabled": true },
  "stats": { "anchors": 847, "tokens": 1200000, "ai_pct": 42 },
  "anchors": [
    {
      "sha": "a1b2c3d",
      "timestamp": "{timestamp}",
      "subject": "fix auth middleware",
      "tools": ["claude"],
      "tokens": 12000,
      "sessions": [ { "id": "{uuid}", "intent": "..." } ]
    }
  ]
}
```

**Exit code:** `0`.

### Disabled-project variant

**Example output:**
```json
{ "project": { "path": "$REPO", "enabled": false }, "anchors": [] }
```
**Exit code:** `0`.

---

## Outside any repo

### Invocation
`oobo` (from `$HOME`, or anywhere not inside a git repo)

**Behavior:** Print an error message with a hint to cd into a repo.

**Example output (stderr):**
```
oobo: not inside a git repository.
      cd into a project and run 'oobo enable', or 'oobo setup' to get started.
```

**JSON mode (`oobo --json`):**
```json
{ "error": "not inside a git repository", "hint": "oobo setup" }
```

**Exit code:** `1`.

---

## Inside a repo that was moved / renamed

### Invocation
`oobo` (after the project folder was renamed from `/a/b` to `/a/c`)

**Behavior:** Project is resolved by `remote_url` first, then `initial_commit_sha` — NOT by path. The old row is found, its `primary_path` is updated to the new location, and the old path is appended to `historical_paths`. No user-visible churn; the TUI opens as normal with all prior anchors intact.

**Side effects:**
- UPDATE on `projects` setting `primary_path` to `$CWD` and appending old path to `historical_paths`.

---

## Invariants

- `oobo --agent` in a repo produces agent-mode anchor listing (default limit 50).
- `oobo --json` in a repo produces JSON anchor listing (default limit 50).
- `oobo > /tmp/out.txt` in a repo → non-TTY → `--agent` output written to the file.
- Unknown subcommands produce clap errors (no git passthrough).
- Running `oobo` on a disabled project NEVER emits a TUI or refreshes the index.
- Running `oobo` outside a repo always exits `1` with a hint message.
