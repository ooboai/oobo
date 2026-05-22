# `oobo delta`

Compare two anchors and get a narrative summary of what changed between them. Requires an API key (the comparison is performed server-side).

Positional args (all optional):
- `<anchor_sha>` — commit to inspect (defaults to HEAD).
- `<previous_sha>` — commit to compare against (auto-resolved if omitted).

Flags:
- `--full` — include detailed sessions, decisions, and techniques in the response.

---

## Default (HEAD vs previous)

### Invocation
`oobo delta`

**Context:** Inside an enabled repo, API key configured, at least 2 anchors exist.

**Behavior:** Resolves HEAD, sends to the remote delta endpoint which finds the previous anchor automatically, returns a comparison.

**Example output (agent mode auto-detected via non-TTY):**
```
current  a1b2c3d  [feature/moderate]  Added auth module
previous d4e5f6g  [fix/trivial]       Fixed typo in README

category:   fix → feature
complexity: trivial → moderate
new areas:  auth
techniques: JWT
new files:  src/auth.rs
continued:  src/main.rs

Moved from bugfix to feature work.
```

**Exit code:** `0`.

**Side effects:** One HTTPS POST to `{api_url}/anchors/delta`.

---

## Explicit anchor SHA

### Invocation
`oobo delta abc123`

**Behavior:** Compare anchor `abc123` against its auto-detected predecessor.

**Exit code:** `0`.

---

## Explicit pair

### Invocation
`oobo delta abc123 def789`

**Behavior:** Compare anchor `abc123` against anchor `def789` specifically.

**Exit code:** `0`.

---

## `--full` flag

### Invocation
`oobo delta --full`

**Behavior:** Same as default but the request tells the backend to include extra detail blocks (sessions, decisions, techniques). Pretty mode shows additional "Current (detail)" and "Previous (detail)" sections.

**Exit code:** `0`.

---

## JSON mode

### Invocation
`oobo delta --json`

**Example output:**
```json
{
  "current": {
    "sha": "a1b2c3d",
    "message": "feat: add auth",
    "author": "dev",
    "timestamp": "2026-05-20T10:00:00Z",
    "headline": "Added auth module",
    "category": "feature",
    "complexity": "moderate"
  },
  "previous": {
    "sha": "d4e5f6g",
    "message": "fix: typo",
    "author": "dev",
    "timestamp": "2026-05-19T09:00:00Z",
    "headline": "Fixed typo in README",
    "category": "fix",
    "complexity": "trivial"
  },
  "changes": {
    "category_shift": { "from": "fix", "to": "feature" },
    "complexity_shift": { "from": "trivial", "to": "moderate" },
    "new_areas": ["auth"],
    "new_techniques": ["JWT"],
    "files_new": ["src/auth.rs"],
    "files_continued": ["src/main.rs"],
    "narrative": "Moved from bugfix to feature work."
  }
}
```

**Exit code:** `0`.

---

## Agent mode

### Invocation
`oobo delta abc123 --agent`

**Behavior:** Compact text columns. One line per anchor summary, then change fields, then narrative.

**Exit code:** `0`.

---

## Pretty / TTY mode

### Invocation
`oobo delta` (with TTY)

**Behavior:** Colored output with bold labels, ANSI colors for shift arrows, green for new files, dim for continued files.

**Exit code:** `0`.

---

## Error cases

### No API key configured
`oobo delta`

**Context:** No API key in settings, no `OOBO_SECRET_KEY` env var.

**Example output (stderr):**
```
error: oobo delta requires an API key. run: oobo settings set key <KEY>
```

**Exit code:** `2`.

### Not inside a git repo
`oobo delta`

**Context:** Invoked outside any git repository.

**Example output (stderr):**
```
oobo: not inside a git repository.
```

**Exit code:** `1`.

### Remote failure (timeout / 5xx)
`oobo delta`

**Behavior:** Hard fail with the HTTP error.

**Example output (stderr):**
```
Error: delta failed: request timed out
```

**Exit code:** non-zero (error propagation).

### No delta data for SHA
`oobo delta abc123`

**Behavior:** Backend returns an empty response (no matching anchors in its DB).

**Example output:**
```
no delta data for abc123
```

**Exit code:** `0`.

---

## Invariants

- `--json` output always parses per `jq '.'` and contains at minimum `current`, `previous`, and `changes` keys (any may be null).
- `--agent` output never contains ANSI escapes.
- Without `--full`, `current_detail` and `previous_detail` are null in JSON.
- The command NEVER writes to the local repo or orphan branch — it is read-only (aside from the network POST).
- SHA resolution uses `git rev-parse` so prefixes, tags, and branch names all work as `<anchor_sha>`.
