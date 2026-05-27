# `oobo search`

Semantic code search powered by sonar (hybrid BM25 + vector). Searches actual source code in the current repo or a specified path.

Positional arg:
- `<query>` - natural language or code query.

Flags:
- `-p, --path <PATH>` - path to directory or git URL (default: repo root or `.`)
- `--git-ref <ref>` - branch or tag to clone (git URLs only)
- `-k, --top-k <N>` - number of results (default 5)
- `-m, --mode <MODE>` - `hybrid`, `semantic`, or `bm25` (default `hybrid`)
- `--content <TYPE>` - `code`, `docs`, `config`, or `all` (default `code`)

---

## Basic code search - agent mode

### Invocation
`oobo search "auth middleware" --agent`

**Behavior:** One result per line: file path, line range, score, snippet preview.

**Example output:**
```
src/auth.rs L10-45 (0.85) pub fn authenticate(req: &Request) -> Result<User, AuthError>...
src/middleware.rs L22-38 (0.72) fn auth_middleware(next: Handler) -> Handler...
```

**Exit code:** `0` if index built successfully (even with zero results).

---

## JSON mode

### Invocation
`oobo search "parse config" --json`

**Example output:**
```json
{
  "query": "parse config",
  "total_hits": 2,
  "results": [
    {
      "file": "src/config.rs",
      "lines": [10, 45],
      "language": "rust",
      "score": 0.91,
      "snippet": "pub fn parse_config(path: &Path)..."
    }
  ]
}
```

**Exit code:** `0`.

---

## Flags

### Top-k
`oobo search "foo" -k 10 --agent`

### Mode
`oobo search "foo" --mode bm25 --agent`

### Content type
`oobo search "foo" --content docs --agent`

### Path
`oobo search "foo" -p /some/path --agent`

---

## Empty / no-result cases

### No supported files
`oobo search "foo" --agent` (in an empty directory)

**Exit code:** `1` with error on stderr.

### Zero hits
`oobo search "completely nonexistent xyzzy" --agent`

**Behavior:** Empty output, exit `0`.
