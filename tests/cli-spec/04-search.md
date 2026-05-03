# `oobo search`

Find any past session across all projects. Search is local-first and reads the local anchor DB. When an API key is configured, remote results are fetched from the backend and merged by score.

Positional arg:
- `<query>` — free-text query. Multi-word queries are treated as AND across terms by default. Quoted substrings match literally.

Flags:
- `--local` — local DB only (default when no API key is configured).
- `--remote` — remote server only (requires `settings set key <...>`).
- `--both` — local + remote merged (default when API key is configured).
- `--since <duration|iso>` — time window.
- `--project <name>` — scope to one project.
- `--tool <name>` — scope to one tool.
- `--limit N` (default `20`).

---

## Basic search — TTY / pretty

### Invocation
`oobo search "auth middleware"`

**Behavior:** Colored, paged hit list. Each hit shows: project, tool, relative time, short intent, and a snippet with the query term highlighted.

**Example output (shape):**
```
oobo-cli · claude · 2m     fix auth middleware
  "the auth middleware drops the token on refresh when..."
  anchor a1b2c3d · 12k tokens

oobo-cli · cursor · 3h     extract payment adapter
  "middleware chain refactored so auth runs before..."
  anchor e1f2d3c · 28k tokens
```

**Exit code:** `0` if at least one hit, `0` if zero hits (NOT an error — emit "no results" footer).

---

## Agent mode

### Invocation
`oobo search "auth middleware" --agent`

**Behavior:** One line per hit. Columns:

1. Anchor short SHA (7 chars) or `-` if the hit is a session not linked to any commit.
2. Tool name (or `-`).
3. Tokens (human-readable, e.g. `12k`).
4. Relative time.
5. One-line snippet (truncated to 60 chars, no surrounding quotes).

Project column prepended when searching across multiple projects (outside a repo, or with no `--project`).

**Example output (inside a repo):**
```
a1b2c3d claude 12k 2m   fix auth middleware: token drops on refresh when...
e1f2d3c cursor 28k 3h   extract payment adapter: middleware chain refactored...
```

**Example output (outside a repo):**
```
oobo-cli  a1b2c3d claude 12k 2m   fix auth middleware: token drops on refresh...
my-app    d4e5f6g gemini 31k 4h   add rate limiter: per-endpoint limits for /api...
```

**Exit code:** `0`.

---

## JSON mode

### Invocation
`oobo search "auth middleware" --json`

**Example output:**
```json
{
  "query": "auth middleware",
  "sources": ["local"],
  "total_hits": 2,
  "hits": [
    {
      "project": { "id": "0f5c...", "name": "oobo-cli" },
      "anchor_sha": "a1b2c3d",
      "session_id": "{uuid}",
      "tool": "claude",
      "tokens": 12000,
      "timestamp": "{timestamp}",
      "intent": "fix auth middleware",
      "snippet": "the auth middleware drops the token on refresh when...",
      "score": 0.91
    }
  ]
}
```

`"sources"` lists which backends contributed: any non-empty subset of `["local", "remote"]`. `"score"` is relative within a single call (0.0–1.0).

**Exit code:** `0`.

---

## Local-only / remote-only / both

### Default with no API key
`oobo search "foo"`

**Behavior:** Implicit `--local`. `sources` in JSON = `["local"]`.

### Default with API key
`oobo search "foo"` (after `oobo settings set key sk_...`)

**Behavior:** Implicit `--both`. Local hits and remote hits are merged by descending `score`. `sources = ["local", "remote"]` when the remote call succeeds.

### Explicit local only
`oobo search "foo" --local`

**Behavior:** Skip the remote call even if an API key is configured.

### Explicit remote only
`oobo search "foo" --remote`

**Behavior:** Skip local search. If no API key is configured → exit `2` with `error: --remote requires an API key. run: oobo settings set key <...>`.

### `--both` without an API key
`oobo search "foo" --both`

**Behavior:** Exit `2` with `error: --both requires an API key. run: oobo settings set key <...>`.

---

## Filters

### `--project <name>`
`oobo search "foo" --project oobo-cli --agent`

**Behavior:** Restrict hits to that project. Resolution: exact name, then fuzzy match (Levenshtein ≤ 2). Ambiguous match → exit `2` with listing.

### `--tool <name>`
`oobo search "foo" --tool claude --agent`

### `--since 7d`
`oobo search "foo" --since 7d --agent`

### `--limit N`
`oobo search "foo" --limit 5 --agent`

---

## Empty / no-result cases

### Zero hits
`oobo search "completely nonexistent phrase" --agent`

**Behavior:** Emit nothing to stdout, exit `0`.

**Example output:**
```
```

(empty, with a final newline)

### Zero hits, pretty mode
`oobo search "completely nonexistent phrase"`

**Example output:**
```
no results for "completely nonexistent phrase"
```
**Exit code:** `0`.

### Zero hits, JSON mode
**Example output:**
```json
{ "query": "completely nonexistent phrase", "sources": ["local"], "total_hits": 0, "hits": [] }
```

---

## Error cases

### Remote failure with `--both`
`oobo search "foo" --both` (API key set, but remote returns 5xx or times out)

**Behavior:** Emit local results with a warning prefix; never fail the whole command on remote failure.

**Example output (pretty, with warning on stderr):**
```
stderr: warning: remote search failed: request: operation timed out. showing local results only.
stdout: [local hits...]
```

**Exit code:** `0`.

### Remote failure with `--remote` only
`oobo search "foo" --remote`

**Behavior:** Hard fail.

**Example output (stderr):**
```
error: remote search failed: request: operation timed out
```
**Exit code:** `1`.

### Empty query
`oobo search ""`

**Example output (stderr):**
```
error: query cannot be empty
```
**Exit code:** `2`.

### Missing query
`oobo search`

**Behavior:** Clap's required-arg error.

**Example output (stderr):**
```
error: the following required arguments were not provided:
  <query>

Usage: oobo search <query> [OPTIONS]
```
**Exit code:** `2`.

---

## Invariants

- `--agent` output never contains ANSI escapes.
- `--json` output always parses per `jq '.'` and has `total_hits == hits.length`.
- Remote failures with `--both` NEVER cause a non-zero exit.
- Outside a repo, the listing always includes a `project` column (`--agent`) or `"project"` field (`--json`).
- `--limit 0` returns zero hits and exits `0`.
