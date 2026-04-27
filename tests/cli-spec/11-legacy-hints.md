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
| `anchor scan` | indexing is automatic now. for a forced reindex: `anchor setup --reindex` | `anchor setup --reindex` |
| `anchor index` | indexing is automatic now. for a forced reindex: `anchor setup --reindex` | `anchor setup --reindex` |
| `anchor sessions [...]` | sessions are shown inside `anchor anchors show <sha>` or `anchor search`. | `anchor anchors` |
| `anchor projects [...]` | manage projects via `anchor setup`; view them with `anchor` (outside a repo). | `anchor setup` |
| `anchor ignore` | use `anchor disable` instead. | `anchor disable` |
| `anchor unignore` | use `anchor enable` instead. | `anchor enable` |
| `anchor sync [...]` | removed. team sync is Git-first (orphan branch). use `anchor settings` for API key. | `anchor settings` |
| `anchor transparency on\|off` | use `anchor settings set transparency on\|off` (advanced). | `anchor settings` |
| `anchor auth [...]` | `anchor settings set key <your-key>`. | `anchor settings` |
| `anchor card` | removed in 1.0. | (no mapped command) |
| `anchor dash` | removed in 1.0; visit `anchor` in repo for the TUI. | `anchor` |
| `anchor sources` | removed in 1.0; run `anchor setup --repair` to re-detect tool paths. | `anchor setup --repair` |
| `anchor inspect` | removed in 1.0; run `anchor setup --repair` for diagnostics. | `anchor setup --repair` |
| `anchor stats` | stats are inline in the anchor view and in `anchor anchors show <sha>`. | `anchor anchors` |
| `anchor agent` | removed; use the global flag `--agent` instead. | `anchor --agent` |
| `anchor share [...]` | removed; use `anchor anchors show <sha> --json` to get redacted, pipeable output. | `anchor anchors` |
| `anchor export [...]` | removed; use `anchor anchors show <sha> --json`. | `anchor anchors` |
| `anchor version` | use `anchor --version`. | `anchor --version` |
| `anchor doctor` | removed; run `anchor setup --repair`. | `anchor setup --repair` |

---

## Examples

### TTY interactive hint

#### Invocation
`anchor scan`

**Example output (stderr):**
```
anchor: 'scan' was removed in 1.0.
      indexing is automatic now. for a forced reindex: anchor setup --reindex

run 'anchor setup --reindex' now? [Y/n]:
```

User types `y` (or Enter): invocation continues as `anchor setup --reindex` (same process, exec'd).
User types `n`: exit `2`, nothing happens.

#### Invocation
`anchor card`

**Example output (stderr):**
```
anchor: 'card' was removed in 1.0.
      (no replacement.)
```
**Exit code:** `2`.

### Non-TTY / agent hint

#### Invocation
`anchor ignore` (with stdout redirected or `--agent`)

**Example output (stderr):**
```
anchor: 'ignore' was removed in 1.0. use 'anchor disable' instead.
```
**Exit code:** `2`.

The hint NEVER auto-executes in non-TTY mode. Scripts that relied on the removed command must be updated explicitly.

### Hint with args passed through

#### Invocation
`anchor scan --force`

**Example output (stderr, TTY):**
```
anchor: 'scan' was removed in 1.0.
      indexing is automatic now. for a forced reindex: anchor setup --reindex

run 'anchor setup --reindex' now? [Y/n]:
```

On `y`, the trailing args (`--force`) are DROPPED — they're not guaranteed to map cleanly. This is a deliberate design choice: the hint is always exact, never best-effort.

### Hint for a subcommand form

#### Invocation
`anchor sessions list`

**Example output (stderr):**
```
anchor: 'sessions' was removed in 1.0.
      sessions are shown inside 'anchor anchors show <sha>' or 'anchor search'.

run 'anchor anchors' now? [Y/n]:
```

### Hint NOT matched — fall through to clap

#### Invocation
`anchor whatever`

**Example output (stderr):**
```
error: unrecognized subcommand 'whatever'

did you mean one of:
  anchors, blame, search, enable, disable, alias, setup, settings, update

Usage: anchor [OPTIONS] [COMMAND]
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
