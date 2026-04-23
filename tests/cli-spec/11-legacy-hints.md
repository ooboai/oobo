# Legacy command hints (0.1.x → 1.0.0)

After the v1.0 consolidation, many 0.1.x commands no longer exist. Rather than forwarding them (which would bloat the binary and confuse muscle memory), we intercept at clap's error handler and emit a one-liner HINT pointing to the new way.

## Hint semantics

1. Clap returns "unrecognized subcommand" for the removed command.
2. Our error handler checks the typed verb against a hint table.
3. If matched:
   - **TTY:** print the hint to stderr + an interactive prompt: "run `<new-cmd>` now? [Y/n]".
     - On Y / Enter → exec the mapped command with remaining argv.
     - On n → exit with code `2`.
   - **Non-TTY / `--agent`:** print the hint to stderr, exit code `2`, NEVER auto-execute.
4. If NOT matched (unknown command not in the legacy list) → fall through to clap's default "did you mean" + exit `2`.

Hints are the ONLY backward-compat surface. No shim commands, no auto-forward. This hint system is removed entirely in v1.1.0+.

## Hint table

| Legacy command | Hint | Mapped interactive run |
|---|---|---|
| `oobo scan` | indexing is automatic now. for a forced reindex: `oobo setup --reindex` | `oobo setup --reindex` |
| `oobo index` | indexing is automatic now. for a forced reindex: `oobo setup --reindex` | `oobo setup --reindex` |
| `oobo sessions [...]` | sessions are shown inside `oobo anchors show <sha>` or `oobo search`. | `oobo anchors` |
| `oobo projects [...]` | manage projects via `oobo setup`; view them with `oobo` (outside a repo). | `oobo setup` |
| `oobo ignore` | use `oobo disable` instead. | `oobo disable` |
| `oobo unignore` | use `oobo enable` instead. | `oobo enable` |
| `oobo sync [...]` | sync is configured via `oobo settings`. set your API key: `oobo settings set key <...>`. | `oobo settings` |
| `oobo transparency on\|off` | use `oobo settings set transparency on\|off` (advanced). | `oobo settings` |
| `oobo auth [...]` | `oobo settings set key <your-key>`. | `oobo settings` |
| `oobo card` | removed in 1.0. | (no mapped command) |
| `oobo dash` | removed in 1.0; visit `oobo` in repo for the TUI. | `oobo` |
| `oobo sources` | removed in 1.0; run `oobo setup --repair` to re-detect tool paths. | `oobo setup --repair` |
| `oobo inspect` | removed in 1.0; run `oobo setup --repair` for diagnostics. | `oobo setup --repair` |
| `oobo stats` | stats are inline in the anchor view and in `oobo anchors show <sha>`. | `oobo anchors` |
| `oobo agent` | removed; use the global flag `--agent` instead. | `oobo --agent` |
| `oobo share [...]` | removed; use `oobo anchors show <sha> --json` to get redacted, pipeable output. | `oobo anchors` |
| `oobo export [...]` | removed; use `oobo anchors show <sha> --json`. | `oobo anchors` |
| `oobo version` | use `oobo --version`. | `oobo --version` |
| `oobo doctor` | removed; run `oobo setup --repair`. | `oobo setup --repair` |

---

## Examples

### TTY interactive hint

#### Invocation
`oobo scan`

**Example output (stderr):**
```
oobo: 'scan' was removed in 1.0.
      indexing is automatic now. for a forced reindex: oobo setup --reindex

run 'oobo setup --reindex' now? [Y/n]:
```

User types `y` (or Enter): invocation continues as `oobo setup --reindex` (same process, exec'd).
User types `n`: exit `2`, nothing happens.

#### Invocation
`oobo card`

**Example output (stderr):**
```
oobo: 'card' was removed in 1.0.
      (no replacement.)
```
**Exit code:** `2`.

### Non-TTY / agent hint

#### Invocation
`oobo ignore` (with stdout redirected or `--agent`)

**Example output (stderr):**
```
oobo: 'ignore' was removed in 1.0. use 'oobo disable' instead.
```
**Exit code:** `2`.

The hint NEVER auto-executes in non-TTY mode. Scripts that relied on the removed command must be updated explicitly.

### Hint with args passed through

#### Invocation
`oobo scan --force`

**Example output (stderr, TTY):**
```
oobo: 'scan' was removed in 1.0.
      indexing is automatic now. for a forced reindex: oobo setup --reindex

run 'oobo setup --reindex' now? [Y/n]:
```

On `y`, the trailing args (`--force`) are DROPPED — they're not guaranteed to map cleanly. This is a deliberate design choice: the hint is always exact, never best-effort.

### Hint for a subcommand form

#### Invocation
`oobo sessions list`

**Example output (stderr):**
```
oobo: 'sessions' was removed in 1.0.
      sessions are shown inside 'oobo anchors show <sha>' or 'oobo search'.

run 'oobo anchors' now? [Y/n]:
```

### Hint NOT matched — fall through to clap

#### Invocation
`oobo whatever`

**Example output (stderr):**
```
error: unrecognized subcommand 'whatever'

did you mean one of:
  anchors, blame, search, enable, disable, alias, setup, settings, update

Usage: oobo [OPTIONS] [COMMAND]
For more information, try '--help'.
```
**Exit code:** `2`.

(If `whatever` is a real git subcommand, see `10-git-passthrough.md` — it's forwarded.)

---

## Invariants

- Every legacy verb in the table above produces a stable hint message (stable across 1.x patch releases).
- In non-TTY mode the hint NEVER auto-executes. Scripts must be updated.
- Unrecognized commands that are NOT in the hint table AND NOT real git verbs → clap error, exit `2`.
- The hint system is removed in 1.1.0+. Warning is printed in every hint: `(this hint will be removed in 1.1.0)`.
