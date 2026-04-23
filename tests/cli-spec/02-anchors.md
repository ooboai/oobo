# `oobo anchors`

See the memory: a list of commits enriched with their AI sessions, token usage, and attribution. Alias: `oobo a`. This is the flagship view. `oobo anchors show <sha>` is the drill-down.

Positional arg is a subcommand (`show`) or nothing.

Common flags:

- `--limit N` (default `50`) — how many anchors to return.
- `--since <timestamp|duration>` — only anchors at or after this point. Accepts ISO-8601 (`2026-04-22T00:00:00Z`) or relative (`24h`, `7d`, `1mo`).
- `--tool <name>` — filter by tool (claude, cursor, gemini, codex, ...).
- `--project <name|path>` — filter to a specific project. Valid only outside a repo (inside, the current repo is implied).

---

## List — TTY / pretty mode

### Invocation
`oobo anchors`

**Context:** inside an enabled repo with anchors.

**Behavior:** Pretty table with colors. Columns: relative time, subject, tools, tokens, session count. Most recent first. Pager on stdout if result count > terminal height.

**Example output (shape):**
```
  2m    fix auth middleware             claude · 12k · 1 session
  18m   add rate limiter                gemini · 31k · 2 sessions
  1h    wip                             (local only)
  3h    extract payment adapter         cursor · 28k · 1 session
```

**Exit code:** `0`.

---

## List — agent mode

### Invocation
`oobo anchors --agent`

**Behavior:** Fixed-column, one line per anchor. Columns, in order:

1. Short SHA (7 chars).
2. Relative time (`2m`, `18m`, `1h`, `3h`, `2d`, `3w`, `4mo`, `1y`).
3. Commit subject (truncated to 40 chars, no ellipsis, padded with spaces).
4. Primary tool name (or `-` if none).
5. Total tokens in human-readable form (`12k`, `1.2M`).
6. Session count suffix (`1s`, `3s`, or `-` if zero).

Columns separated by **one or more spaces**. Never tab-separated. No header, no totals.

**Example output:**
```
a1b2c3d 2m   fix auth middleware                      claude 12k  1s
d4e5f6g 18m  add rate limiter                         gemini 31k  2s
7a8b9c0 1h   wip                                      -      -    -
e1f2d3c 3h   extract payment adapter                  cursor 28k  1s
```

**Exit code:** `0`.

---

## List — JSON mode

### Invocation
`oobo anchors --json`

**Behavior:** Emit a flat JSON array of anchors (no envelope). Per-anchor shape:

```json
{
  "sha": "a1b2c3d4e5f6...",
  "parents": ["..."],
  "timestamp": "{timestamp}",
  "author": { "name": "Teddy", "email": "teddy@example.com" },
  "subject": "fix auth middleware",
  "body": "...multi-line commit body...",
  "tools": ["claude"],
  "tokens": { "input": 8000, "output": 4000, "cache_read": 1024, "cache_write": 512, "total": 12000 },
  "cost_usd": 0.042,
  "sessions": [
    {
      "id": "{uuid}",
      "tool": "claude",
      "intent": "the auth middleware drops the token on refresh when...",
      "started_at": "{timestamp}",
      "ended_at": "{timestamp}",
      "tokens": { /* same shape */ }
    }
  ],
  "attribution": { "ai_lines": 42, "human_lines": 18, "ai_pct": 70 }
}
```

**Exit code:** `0`.

---

## Filters

### `--limit N`
`oobo anchors --agent --limit 3`

**Behavior:** Emit at most N rows. `N = 0` → empty output, exit `0` (not an error).

**Example output:**
```
a1b2c3d 2m   fix auth middleware                      claude 12k  1s
d4e5f6g 18m  add rate limiter                         gemini 31k  2s
7a8b9c0 1h   wip                                      -      -    -
```

### `--since <duration>`
`oobo anchors --agent --since 24h`

**Behavior:** Only anchors with `timestamp >= now() - 24h`. Accepts `s`, `m`, `h`, `d`, `w`, `mo`, `y` suffixes. Invalid duration → exit `2` with error on stderr.

### `--since <ISO-8601>`
`oobo anchors --agent --since 2026-04-01T00:00:00Z`

**Behavior:** Parse with chrono. Invalid ISO string → exit `2`.

### `--tool <name>`
`oobo anchors --agent --tool claude`

**Behavior:** Only anchors whose `tools` array contains the given tool. Case-insensitive exact match.

### Combined filters
`oobo anchors --agent --tool claude --since 7d --limit 10`

**Behavior:** AND semantics. Filters compose.

### `--project` outside a repo
`oobo anchors --agent --project oobo-cli`

**Behavior:** From anywhere (not inside a repo), show anchors for the named project. Resolved by project name first, then by path. Ambiguous name → exit `2` with `error: multiple projects match 'oobo-cli'` and a listing.

### `--project` inside a repo
`oobo anchors --project other-project`

**Behavior:** ERROR. Inside a repo the project is implied; `--project` is rejected.

**Example output (stderr):**
```
error: --project is not allowed inside a repo (current project is '$PROJECT_NAME')
```
**Exit code:** `2`.

---

## Drill-down — `anchors show <sha>`

### Invocation
`oobo anchors show a1b2c3d`

**Behavior:** Show ONE anchor in depth. Pretty mode prints a paged document with sections: commit metadata, the commit diff (abbreviated by default), linked sessions (one collapsible block each), tokens + cost breakdown, per-line attribution.

**Example output (pretty, shape):**
```
a1b2c3d — fix auth middleware
Teddy <teddy@example.com>  ·  2m ago
─────────────────────────────────────────────────────────
SUBJECT
  fix auth middleware

TOOLS     claude
TOKENS    12,000 (input 8k · output 4k · cache 1.5k)
COST      $0.042
ATTRIB    42 AI lines · 18 human lines · 70% AI

SESSIONS
  ● claude · 45m · 12k tokens · $0.042
    "the auth middleware drops the token on refresh when..."

    [press enter to view full transcript]

DIFF
  [abbreviated diff, 10 lines each side]

  [press 'f' for full diff, 'q' to close]
```

**Exit code:** `0` on clean quit.

### `oobo anchors show <sha> --agent`

**Behavior:** Flat, minimal. One section per line with `key: value` shape. Transcript is NOT inlined in agent mode — the session ID is emitted so the agent can fetch it separately if needed.

**Example output:**
```
sha:        a1b2c3d4e5f6...
subject:    fix auth middleware
author:     Teddy <teddy@example.com>
timestamp:  2026-04-22T14:03:12Z
tools:      claude
tokens:     12000 (in 8000 / out 4000 / cache 1536)
cost_usd:   0.042
ai_pct:     70
sessions:
  {uuid} claude 45m 12000 "the auth middleware drops..."
```

**Exit code:** `0`.

### `oobo anchors show <sha> --json`

**Behavior:** Full anchor object (same shape as list entries but with full `body`, full transcript per session, and any extra fields like `diff_summary`, `files_changed`, `hunks`).

**Exit code:** `0`.

---

## Error cases

### Unknown SHA
`oobo anchors show nothere`

**Example output (stderr):**
```
error: no anchor found for 'nothere'
```
**Exit code:** `1`.

### Ambiguous SHA prefix
`oobo anchors show a1`

**Example output (stderr):**
```
error: '' matches multiple anchors:
  a1b2c3d  fix auth middleware
  a1f7e2c  refactor token store
```
**Exit code:** `1`.

### Outside a repo without `--project`
`oobo anchors` (from `$HOME`)

**Behavior:** Aggregate across all tracked projects. Each row carries a leading `project` column (agent/pretty) or a `"project"` string field (json). The envelope keys are NOT added in this mode — output stays a flat array in `--json`.

**Example output (`--agent`):**
```
oobo-cli   a1b2c3d 2m   fix auth middleware        claude 12k  1s
my-app     d4e5f6g 18m  add rate limiter           gemini 31k  2s
```

### Disabled project
`oobo anchors` inside a repo where `oobo = off`.

**Example output (pretty):**
```
oobo is disabled for this project. run: oobo enable
```
**Exit code:** `0`.

---

## Invariants

- Bare `oobo --agent` inside a repo ≡ `oobo anchors --agent --limit 50` byte-for-byte.
- The `sha` column in `--agent` output is always 7 chars.
- The `--agent` output never contains ANSI escape codes.
- The `--json` output is always valid per `jq '.'`.
- `anchors show <prefix>` where prefix uniquely identifies an anchor succeeds; ambiguous prefixes fail with a listing.
