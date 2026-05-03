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
| `oobo anchors [...]` | bare 'oobo' shows the memory feed now. use 'oobo anchor show <sha>' to drill in. | `oobo` |
| `oobo a [...]` | bare 'oobo' shows the memory feed now. use 'oobo anchor show <sha>' to drill in. | `oobo` |
| `oobo alias [...]` | removed. oobo is standalone — no git alias needed. | (no mapped command) |
| `oobo scan` | indexing is automatic now. for a forced reindex: `oobo setup --reindex` | `oobo setup --reindex` |
| `oobo index` | indexing is automatic now. for a forced reindex: `oobo setup --reindex` | `oobo setup --reindex` |
| `oobo sessions [...]` | sessions are shown inside `oobo anchor show <sha>` or `oobo search`. | `oobo` |
| `oobo projects [...]` | manage projects via `oobo setup`; view them with `oobo` (outside a repo). | `oobo setup` |
| `oobo ignore` | use `oobo disable` instead. | `oobo disable` |
| `oobo unignore` | use `oobo enable` instead. | `oobo enable` |
| `oobo sync [...]` | removed. team sync is Git-first (orphan branch). use `oobo settings` for API key. | `oobo settings` |
| `oobo transparency on\|off` | use `oobo settings set transparency on\|off` (advanced). | `oobo settings` |
| `oobo auth [...]` | `oobo settings set key <your-key>`. | `oobo settings` |
| `oobo card` | removed in 1.0. | (no mapped command) |
| `oobo dash` | removed in 1.0; visit `oobo` in repo for the TUI. | `oobo` |
| `oobo sources` | removed in 1.0; run `oobo setup --repair` to re-detect tool paths. | `oobo setup --repair` |
| `oobo inspect` | removed in 1.0; run `oobo setup --repair` for diagnostics. | `oobo setup --repair` |
| `oobo stats` | stats are inline in the anchor view and in `oobo anchor show <sha>`. | `oobo` |
| `oobo agent` | removed; use the global flag `--agent` instead. | (no mapped command) |
| `oobo share [...]` | removed; use `oobo anchor show <sha> --json` to get redacted, pipeable output. | `oobo` |
| `oobo export [...]` | removed; use `oobo anchor show <sha> --json`. | `oobo` |
| `oobo version` | use `oobo --version`. | (no mapped command) |
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
      (this hint will be removed in 1.1.0)

run 'oobo setup --reindex' now? [Y/n]:
```

User types `y` (or Enter): invocation continues as `oobo setup --reindex` (same process, exec'd).
User types `n`: exit `2`, nothing happens.

#### Invocation
`oobo card`

**Example output (stderr):**
```
oobo: 'card' was removed in 1.0.
      removed in 1.0.
      (this hint will be removed in 1.1.0)
```
**Exit code:** `2`.

### `oobo anchors` legacy hint

#### Invocation
`oobo anchors`

**Example output (stderr, TTY):**
```
oobo: 'anchors' was removed in 1.0.
      bare 'oobo' shows the memory feed now. use 'oobo anchor show <sha>' to drill in.
      (this hint will be removed in 1.1.0)

run 'oobo' now? [Y/n]:
```

User types `y` (or Enter): invocation continues as bare `oobo`.
User types `n`: exit `2`.

#### Invocation (non-TTY)
`oobo anchors` (with stdout redirected or `--agent`)

**Example output (stderr):**
```
oobo: 'anchors' was removed in 1.0.
      bare 'oobo' shows the memory feed now. use 'oobo anchor show <sha>' to drill in.
      (this hint will be removed in 1.1.0)
```
**Exit code:** `2`.

### `oobo a` legacy hint (short alias)

#### Invocation
`oobo a`

**Example output (stderr, TTY):**
```
oobo: 'a' was removed in 1.0.
      bare 'oobo' shows the memory feed now. use 'oobo anchor show <sha>' to drill in.
      (this hint will be removed in 1.1.0)

run 'oobo' now? [Y/n]:
```

**Exit code:** `2` on `n`, or continues as `oobo` on `y`.

### `oobo alias` legacy hint

#### Invocation
`oobo alias`

**Example output (stderr):**
```
oobo: 'alias' was removed in 1.0.
      removed. oobo is standalone — no git alias needed.
      (this hint will be removed in 1.1.0)
```
**Exit code:** `2`.

No interactive prompt — `alias` has no mapped command.

#### Invocation
`oobo alias install`

**Behavior:** The legacy hint system intercepts `alias` before clap sees `install`. Trailing args are dropped. Same hint output.

**Exit code:** `2`.

### Non-TTY / agent hint

#### Invocation
`oobo ignore` (with stdout redirected or `--agent`)

**Example output (stderr):**
```
oobo: 'ignore' was removed in 1.0.
      use 'oobo disable' instead.
      (this hint will be removed in 1.1.0)
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
      (this hint will be removed in 1.1.0)

run 'oobo setup --reindex' now? [Y/n]:
```

On `y`, the trailing args (`--force`) are DROPPED — they're not guaranteed to map cleanly. This is a deliberate design choice: the hint is always exact, never best-effort.

### Hint for a subcommand form

#### Invocation
`oobo sessions list`

**Example output (stderr):**
```
oobo: 'sessions' was removed in 1.0.
      sessions are shown inside 'oobo anchor show <sha>' or 'oobo search'.
      (this hint will be removed in 1.1.0)

run 'oobo' now? [Y/n]:
```

### Hint NOT matched — fall through to clap

#### Invocation
`oobo whatever`

**Example output (stderr):**
```
error: unrecognized subcommand 'whatever'

Usage: oobo [OPTIONS] [COMMAND]

For more information, try '--help'.
```
**Exit code:** `2`.

---

## Invariants

- Every legacy verb in the table above produces a stable hint message (stable across 1.x patch releases).
- In non-TTY mode the hint NEVER auto-executes. Scripts must be updated.
- Unrecognized commands that are NOT in the hint table → clap error, exit `2`.
- The hint system is removed in 1.1.0+. Warning is printed in every hint: `(this hint will be removed in 1.1.0)`.
