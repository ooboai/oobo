# `oobo blame`

Per-line attribution: who (or which AI tool) wrote each line of a file, at a given commit.

**Strict superset of `git blame`.** Every `git blame` flag is supported and produces identical results to `git blame` for the same flags — plus an additional `ai/human` attribution column. If you `alias git=oobo`, typing `git blame` delegates here and you get more info, never less.

Positional args:
- `<file>` — required path to a tracked file, relative to repo root or absolute.
- `[commit]` — optional commit-ish (SHA, branch, tag); defaults to `HEAD`.

Passthrough flags from `git blame` (not exhaustive): `-L <range>`, `-w`, `--abbrev=N`, `-M`, `-C`, `--root`, `--incremental`, `--line-porcelain`, `--porcelain`, `-s`, `-e`, `--date=<fmt>`, `--since=<date>`, `--until=<date>`, `-b`. These are forwarded verbatim to the underlying git invocation.

oobo-specific flags:
- `--no-ai` — strip the AI attribution column; emit pure `git blame` output.

---

## Pretty / TTY mode

### Invocation
`oobo blame src/main.rs`

**Context:** inside a repo, file exists in HEAD.

**Behavior:** Same shape as `git blame` with colored output + one extra leading column showing who wrote each line: `human:<author>` or `<tool>:<session_short_id>`. If the project is disabled or the line has no anchor yet, the column shows `-`.

**Example output:**
```
a1b2c3d  claude   (Teddy 2m)   1  fn main() {
a1b2c3d  human    (Teddy 2m)   2      let args = parse_args();
d4e5f6g  gemini   (Teddy 18m)  3      let config = load_config();
d4e5f6g  human    (Teddy 18m)  4      run(args, config);
a1b2c3d  claude   (Teddy 2m)   5  }
```

**Exit code:** `0`.

---

## Agent mode

### Invocation
`oobo blame src/main.rs --agent`

**Behavior:** Flat columns, one line per source line. Columns:

1. Commit short SHA (7 chars).
2. Attribution: `human` or `<tool>` (lowercase, trimmed).
3. Author name (no email, no brackets).
4. Line number (right-aligned).
5. Line content, untrimmed.

**Example output:**
```
a1b2c3d claude Teddy     1  fn main() {
a1b2c3d human  Teddy     2      let args = parse_args();
d4e5f6g gemini Teddy     3      let config = load_config();
d4e5f6g human  Teddy     4      run(args, config);
a1b2c3d claude Teddy     5  }
```

**Exit code:** `0`.

---

## JSON mode

### Invocation
`oobo blame src/main.rs --json`

**Behavior:** Emit a JSON document with an ordered array of line entries.

**Example output:**
```json
{
  "file": "src/main.rs",
  "commit": "HEAD",
  "lines": [
    { "line": 1, "sha": "a1b2c3d", "author": { "name": "Teddy", "email": "teddy@example.com" }, "ai": "claude", "session": "{uuid}", "content": "fn main() {" },
    { "line": 2, "sha": "a1b2c3d", "author": { "name": "Teddy", "email": "teddy@example.com" }, "ai": null,     "session": null,      "content": "    let args = parse_args();" }
  ]
}
```

`"ai"` is either the tool name (string) or `null` (human-written). `"session"` is the session UUID that produced the line, or `null`.

**Exit code:** `0`.

---

## Git-blame flag passthrough

### `-L <start>,<end>`
`oobo blame -L 10,20 src/main.rs`

**Behavior:** Like `git blame -L 10,20` plus the AI column. Only lines 10–20 are emitted.

### `-w` (ignore whitespace)
`oobo blame -w src/main.rs`

**Behavior:** Forwarded to git. AI attribution follows whichever commit git attributes the line to.

### `--porcelain`
`oobo blame --porcelain src/main.rs`

**Behavior:** Emit git's porcelain format UNCHANGED — no extra AI column, because porcelain is intended for machine parsing and must round-trip. This mode is for scripts that already parse `git blame --porcelain`. To get machine-readable AI attribution, use `--json` instead.

**Exit code:** `0`.

### `--no-ai`
`oobo blame --no-ai src/main.rs`

**Behavior:** Emit output byte-for-byte identical to `git blame src/main.rs`. No AI column, no extra processing. This is the "I'm scripting against git blame and don't want oobo noise" escape hatch.

**Example output:**
```
a1b2c3d (Teddy 2m)   1) fn main() {
a1b2c3d (Teddy 2m)   2)     let args = parse_args();
d4e5f6g (Teddy 18m)  3)     let config = load_config();
```

**Exit code:** `0`.

---

## Commit argument

### Invocation
`oobo blame src/main.rs a1b2c3d`

**Behavior:** Blame the file at the specified commit, not at HEAD.

### Invocation
`oobo blame src/main.rs main`

**Behavior:** Blame at the tip of `main` branch.

### Invocation
`oobo blame src/main.rs v1.0.0`

**Behavior:** Blame at the `v1.0.0` tag.

### Unknown commit-ish
`oobo blame src/main.rs nothere`

**Example output (stderr):**
```
fatal: no such ref: nothere
```
**Exit code:** `128` (git's exit code — passed through unchanged).

---

## Error cases

### File not tracked / not exists
`oobo blame src/nothing.rs`

**Example output (stderr):**
```
fatal: no such path src/nothing.rs in HEAD
```
**Exit code:** `128`.

### Outside a repo
`oobo blame any-file` (from `$HOME`)

**Example output (stderr):**
```
fatal: not a git repository (or any of the parent directories): .git
```
**Exit code:** `128`.

### Disabled project
`oobo blame src/main.rs` inside a repo where `.oobo/config` has `[project].enabled = false`.

**Behavior:** Works exactly like `git blame`. The AI column is present but every cell is `-` (we still know which commit touched each line; we just don't attribute AI authorship).

**Example output:**
```
a1b2c3d - Teddy     1  fn main() {
a1b2c3d - Teddy     2      let args = parse_args();
```

**Exit code:** `0`.

---

## Invariants

- `oobo blame --no-ai $file` output is byte-for-byte identical to `git blame $file`.
- `oobo blame --porcelain $file` output is byte-for-byte identical to `git blame --porcelain $file`.
- For any file with zero AI sessions linked to any of its commits, `oobo blame $file` differs from `git blame $file` only by added `-` columns on each line (or no difference with `--no-ai`).
- `oobo blame $file --agent` never contains ANSI escape codes.
