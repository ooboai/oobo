# Bare `oobo` — no subcommand

Resolves along two dimensions: (1) inside a git repo or not, (2) output mode (pretty / agent / json). That's the "four quadrants".

`oobo` alone is NEVER forwarded to `git`. Git passthrough only applies to `oobo <git-verb>` forms (see `10-git-passthrough.md`). Someone typing `oobo` with no args is asking for oobo, not for `git`'s usage banner.

---

## Inside a git repo, pretty mode (TTY, no flags, no agent env)

### Invocation
`oobo`

**Context:** `$CWD` is inside a git repo that is `enabled` in oobo. TTY attached. No agent env vars.

**Behavior:** Open the anchor-feed TUI. Keys: `↑` / `↓` navigate, `Enter` drills into `anchors show <sha>` view, `/` opens search, `q` quits. Header shows aggregate stats (anchors, tokens, AI%). If the project is disabled, show the disabled-banner variant (see below).

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

**Behavior:** Byte-for-byte identical to `oobo anchors --agent --limit 50`. Minimal one-line-per-anchor listing. See `02-anchors.md` for full column spec.

**Example output:**
```
a1b2c3d 2m   fix auth middleware        claude 12k 1s
d4e5f6g 18m  add rate limiter           gemini 31k 2s
7a8b9c0 1h   wip                        -      -   -
e1f2d3c 3h   extract payment adapter    cursor 28k 1s
```

**Exit code:** `0`.

**Side effects:** none.

### Disabled-project variant

**Behavior:** Emit one line:
```
oobo disabled for this project. run: oobo enable
```
**Exit code:** `0`.

---

## Inside a git repo, JSON mode

### Invocation
`oobo --json`

**Behavior:** Byte-for-byte identical to `oobo anchors --json --limit 50`. Full structured anchor list for this repo.

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

## Outside any repo, pretty mode (TTY)

### Invocation
`oobo` (from `$HOME`, or anywhere not inside a git repo)

**Behavior:** Open the cross-project TUI — a feed of all tracked projects across all repos on this machine, grouped by recent activity. Same four keys.

**Example output (screen buffer, shape):**
```
┌─ oobo · 12 projects ───────────── 2.1k anchors · 4.3M tok · 51% AI ──┐
│                                                                      │
│  oobo-cli        2m   847 anchors · 1.2M · 42% AI                    │
│  my-app          4h   120 anchors · 230k · 68% AI                    │
│  work-api        1d   412 anchors · 880k · 35% AI (disabled)         │
│                                                                      │
└─ ↑↓ nav · enter open project · / search · q quit ───────────────────┘
```

**Exit code:** `0`.

### Zero projects tracked

**Behavior:** Display a one-line welcome and point to setup. No TUI.

**Example output:**
```
oobo: no projects tracked yet. run:

    oobo setup

to discover projects and AI sessions on this machine.
```

**Exit code:** `0`.

---

## Outside any repo, agent mode

### Invocation
`oobo --agent` (from `$HOME`)

**Behavior:** One line per tracked project. Columns: project name, last-activity relative time, anchor count, total tokens, AI percentage, enabled/disabled flag.

**Example output:**
```
oobo-cli     2m   847 1.2M 42% on
my-app       4h   120 230k 68% on
work-api     1d   412 880k 35% off
```

**Exit code:** `0`.

### Zero projects tracked

**Example output:**
```
no projects tracked. run: oobo setup
```
**Exit code:** `0`.

---

## Outside any repo, JSON mode

### Invocation
`oobo --json` (from `$HOME`)

**Example output:**
```json
{
  "projects": [
    {
      "id": "0f5c...",
      "name": "oobo-cli",
      "path": "/Users/teddy/dev/oobo-cli",
      "remote": "git@github.com:me/oobo-cli.git",
      "enabled": true,
      "last_activity": "{timestamp}",
      "stats": { "anchors": 847, "tokens": 1200000, "ai_pct": 42 }
    }
  ],
  "stats": { "projects": 12, "anchors": 2100, "tokens": 4300000, "ai_pct": 51 }
}
```

**Exit code:** `0`.

### Zero projects tracked

**Example output:**
```json
{ "projects": [], "stats": { "projects": 0, "anchors": 0, "tokens": 0, "ai_pct": 0 } }
```
**Exit code:** `0`.

---

## Inside a repo that was moved / renamed

### Invocation
`oobo` (after the project folder was renamed from `/a/b` to `/a/c`)

**Behavior:** Project is resolved by `remote_url` first, then `initial_commit_sha` — NOT by path. The old row is found, its `primary_path` is updated to the new location, and the old path is appended to `historical_paths`. No user-visible churn; the TUI opens as normal with all prior anchors intact.

**Side effects:**
- UPDATE on `projects` setting `primary_path` to `$CWD` and appending old path to `historical_paths`.

---

## Invariants

- `oobo --agent` in a repo ≡ `oobo anchors --agent --limit 50` (byte-for-byte).
- `oobo --json` in a repo ≡ `oobo anchors --json --limit 50` (byte-for-byte).
- `oobo > /tmp/out.txt` in a repo → non-TTY → `--agent` output written to the file.
- `oobo` NEVER forwards to `git` (passthrough applies only to `oobo <verb>` forms).
- Running `oobo` on a disabled project NEVER emits a TUI or refreshes the index.
