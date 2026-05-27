# Bare `oobo`  --  memory feed

See the memory: committed anchors plus local restorable working memory for the
current repo. This is the flagship view. Bare `oobo` (with optional `--agent`,
`--json`, and filter flags) is the primary entry point. `oobo anchor show <sha>` is
the drill-down for committed anchors.

Filter flags (global on `Cli`):

- `-n, --limit N` (default `50`)  --  how many anchors to return.
- `--since <timestamp|duration>`  --  only anchors at or after this point. Accepts ISO-8601 (`2026-04-22T00:00:00Z`) or relative (`24h`, `7d`, `1mo`).
- `--tool <name>`  --  filter by tool (claude, cursor, gemini, codex, ...).

---

## List  --  TTY / pretty mode

### Invocation
`oobo`

`oobo anchors` is an explicit alias for the same command.

### Invocation
`oobo anchors`

**Context:** inside an enabled repo with anchors.

**Behavior:** Full-screen interactive memory timeline. Inside a repo, committed
anchors and working memory are shown together. Anchors represent committed
memory; working memory rows are uncommitted restorable points.

**Example output (shape):**
```
oobo / my-project  memory
branch main  tree clean  anchors origin
3 committed  1 working   window all   tracking on

› ╰ · 2m   adjust auth prompt handling              2f  1t #4
  │ ● 18m  add rate limiter                         31k  2s
```

**Exit code:** `0`.

---

## List  --  agent mode

### Invocation
`oobo --agent`

**Behavior:** Fixed-column, one line per memory item. Inside a repo this can
include both `anchor` and `shadow` rows. `shadow` is the stable machine label
for local working memory. Columns, in order:

1. Type (`anchor` or `shadow`).
2. Short id (commit SHA or shadow anchor id).
3. Relative time (`2m`, `18m`, `1h`, `3h`, `2d`, `3w`, `4mo`, `1y`).
4. Subject/prompt (truncated to 40 chars, no ellipsis, padded with spaces).
5. Primary tool name (or `-` if none).
6. Total tokens for committed anchors, or `-` for working memory.
7. Session count suffix for committed anchors (`1s`, `3s`) or file/tool summary for working memory (`1f/2t`).

Columns separated by **one or more spaces**. Never tab-separated. No header, no totals.

**Example output:**
```
shadow tfa4069dfa 2m   adjust auth prompt handling              composer -    1f/1t
anchor a1b2c3d    18m  add rate limiter                         gemini   31k  2s
```

**Exit code:** `0`.

---

## List  --  JSON mode

### Invocation
`oobo --json`

**Behavior:** Emit a flat JSON array (no envelope). Each item has `"type":
"anchor"` or `"shadow_anchor"`. Anchor items include committed-anchor fields;
shadow anchor items include local snapshot/session metadata and are present only
inside a repo.
Per-anchor shape:

```json
{
  "type": "anchor",
  "id": "a1b2c3d4e5f6...",
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
  "attribution": { "ai_lines": 42, "human_lines": 18, "ai_pct": 70 },
}
```

Per-working-memory shape:

```json
{
  "type": "shadow_anchor",
  "id": "tfa4069dfa3f775d4",
  "shadow_anchor_id": "tfa4069dfa3f775d4",
  "turn_id": "tfa4069dfa3f775d4",
  "session_id": "{uuid}",
  "turn_index": 1,
  "parent_anchor": "a1b2c3d4e5f6...",
  "timestamp": "{timestamp}",
  "subject": "adjust auth prompt handling",
  "tools": ["composer"],
  "tokens": { "total": 0 },
  "sessions_count": 1,
  "files": 1,
  "tool_calls": 1
}
```

**Exit code:** `0`.

---

## Filters

### `-n N` / `--limit N`
`oobo --agent -n 3`

**Behavior:** Emit at most N rows. `N = 0` → empty output, exit `0` (not an error).

**Example output:**
```
a1b2c3d 2m   fix auth middleware                      claude 12k  1s
d4e5f6g 18m  add rate limiter                         gemini 31k  2s
7a8b9c0 1h   wip                                      -      -    -
```

### `--since <duration>`
`oobo --agent --since 24h`

**Behavior:** Only anchors with `timestamp >= now() - 24h`. Accepts `s`, `m`, `h`, `d`, `w`, `mo`, `y` suffixes. Invalid duration → exit `2` with error on stderr.

### `--since <ISO-8601>`
`oobo --agent --since 2026-04-01T00:00:00Z`

**Behavior:** Parse with chrono. Invalid ISO string → exit `2`.

### `--tool <name>`
`oobo --agent --tool claude`

**Behavior:** Only anchors whose `tools` array contains the given tool. Case-insensitive exact match.

### Combined filters
`oobo --agent --tool claude --since 7d -n 10`

**Behavior:** AND semantics. Filters compose.

---

## Drill-down  --  `oobo anchor show <sha>`

### Invocation
`oobo anchor show a1b2c3d`

**Behavior:** Show ONE anchor in depth. Pretty mode prints a paged document with sections: commit metadata, the commit diff (abbreviated by default), linked sessions (one collapsible block each), tokens + cost breakdown, per-line attribution.

**Example output (pretty, shape):**
```
a1b2c3d  --  fix auth middleware
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

### `oobo anchor show <sha> --agent`

**Behavior:** Flat, minimal. One section per line with `key: value` shape. Transcript is NOT inlined in agent mode  --  the session ID is emitted so the agent can fetch it separately if needed.

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

### `oobo anchor show <sha> --json`

**Behavior:** Full anchor object (same shape as list entries but with full `body`, full transcript per session, and any extra fields like `diff_summary`, `files_changed`, `hunks`).

**Exit code:** `0`.

---

## Error cases

### Unknown SHA
`oobo anchor show nothere`

**Example output (stderr):**
```
error: no anchor found for 'nothere'
```
**Exit code:** `1`.

### Outside a repo
`oobo` (from `$HOME`)

**Behavior:** Print a hint to cd into a repo. No feed is shown.

**Example output (stderr):**
```
oobo: not inside a git repository.
      cd into a project and run 'oobo enable', or 'oobo setup' to get started.
```
**Exit code:** `1`.

### Disabled project
`oobo` inside a repo where `.oobo/config` has `[project].enabled = false`.

**Example output (pretty):**
```
oobo is disabled for this project. run: oobo enable
```
**Exit code:** `0`.

---

## Invariants

- Bare `oobo --agent` inside a repo produces the agent-mode feed (default limit 50).
- The `sha` column in `--agent` output is always 7 chars.
- The `--agent` output never contains ANSI escape codes.
- The `--json` output is always valid per `jq '.'`.
- `oobo anchor show <prefix>` where prefix uniquely identifies an anchor succeeds; ambiguous prefixes fail with a listing.
- `oobo` outside a repo always exits `1` with a hint.