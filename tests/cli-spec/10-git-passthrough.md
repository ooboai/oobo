# Git passthrough — removed

Git passthrough was removed in 1.0. `oobo` is no longer a transparent decorator around `git`. Unknown subcommands are NOT forwarded to git — they produce clap errors.

Users who want git functionality should invoke `git` directly.

---

## Unknown subcommand → clap error

### Invocation
`oobo status`

**Behavior:** `status` is not a recognized oobo subcommand. Clap returns an error. It is NOT forwarded to `git`.

**Example output (stderr):**
```
error: unrecognized subcommand 'status'

Usage: oobo [OPTIONS] [COMMAND]

For more information, try '--help'.
```
**Exit code:** `2`.

---

### Invocation
`oobo log --oneline -n 5`

**Behavior:** `log` is not a recognized oobo subcommand. Clap error.

**Example output (stderr):**
```
error: unrecognized subcommand 'log'

Usage: oobo [OPTIONS] [COMMAND]

For more information, try '--help'.
```
**Exit code:** `2`.

---

### Invocation
`oobo push`

**Behavior:** `push` is not a recognized oobo subcommand. Clap error (not forwarded to `git push`).

**Example output (stderr):**
```
error: unrecognized subcommand 'push'

Usage: oobo [OPTIONS] [COMMAND]

For more information, try '--help'.
```
**Exit code:** `2`.

---

### Invocation
`oobo nonexistent-verb`

**Behavior:** Clap error. Not forwarded to git.

**Example output (stderr):**
```
error: unrecognized subcommand 'nonexistent-verb'

Usage: oobo [OPTIONS] [COMMAND]

For more information, try '--help'.
```
**Exit code:** `2`.

---

## Legacy commands → hints (not git)

Commands that were removed in 1.0 (like `scan`, `sessions`, `projects`) are intercepted by the legacy hint system (see `11-legacy-hints.md`) BEFORE clap parsing. They produce a hint message, not a clap error and not a git invocation.

---

## Invariants

- NO subcommand is ever forwarded to `git`. oobo is a standalone binary.
- Unknown subcommands that are NOT in the legacy hint table produce clap's standard "unrecognized subcommand" error with exit `2`.
- Unknown subcommands that ARE in the legacy hint table produce a hint message with exit `2` (see `11-legacy-hints.md`).
- `oobo blame` is an oobo command (not a git passthrough). See `03-blame.md`.
