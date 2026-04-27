# `anchor update`

Self-update. Checks GitHub (or the configured update channel) for a newer release, downloads it, swaps the binary in place, and verifies the new binary runs.

Flags:
- `--check` — only check; don't download or install.
- `--channel <stable|beta>` — pick a release channel (default: `stable`).
- `--force` — overwrite even if the local binary is already at the latest version (useful for repair).
- `--yes` / `-y` — skip the confirmation prompt.

---

## `anchor update` — interactive

### Invocation (upgrade available)
`anchor update`

**Behavior:**
1. Query update manifest from GitHub Releases (or configured remote).
2. Compare to `env!("CARGO_PKG_VERSION")`.
3. If newer: prompt `[Y/n]`, download, verify checksum, swap.
4. Verify new binary by running `anchor --version`.

**Example output:**
```
checking for updates...
  current: anchor 1.0.0
  latest:  anchor 1.1.0 (2026-05-12)

changelog: https://github.com/oobo/oobo-cli/releases/tag/v1.1.0

install? [Y/n]: y
  downloading anchor-1.1.0-aarch64-darwin.tar.gz...
  verifying checksum... ok
  swapping binary at /usr/local/bin/anchor...
  verifying new binary... ok

updated: anchor 1.0.0 → 1.1.0
```

**Exit code:** `0` on success, `1` on failure (network, checksum, swap), `130` on Ctrl-C.

### Invocation (already up-to-date)
`anchor update`

**Example output:**
```
anchor 1.0.0 is the latest stable release.
```
**Exit code:** `0`.

### Invocation (user declines)
`anchor update` → answer `n` at the prompt.

**Example output:**
```
install? [Y/n]: n
no changes made.
```
**Exit code:** `0`.

---

## `anchor update --check`

### Invocation
`anchor update --check`

**Behavior:** Print the current and latest versions. NEVER downloads or installs. Safe for cron.

**Example output (pretty, upgrade available):**
```
current: anchor 1.0.0
latest:  anchor 1.1.0
an update is available. run: anchor update
```

**Example output (pretty, up-to-date):**
```
anchor 1.0.0 is the latest stable release.
```

### Invocation
`anchor update --check --agent`

**Example output (upgrade available):**
```
1.0.0 1.1.0 update-available
```

**Example output (up-to-date):**
```
1.0.0 1.0.0 up-to-date
```

### Invocation
`anchor update --check --json`

**Example output:**
```json
{
  "current": "1.0.0",
  "latest": "1.1.0",
  "status": "update-available",
  "changelog_url": "https://github.com/oobo/oobo-cli/releases/tag/v1.1.0",
  "published_at": "{timestamp}"
}
```

Possible `status` values: `up-to-date`, `update-available`, `ahead-of-latest` (current > latest, e.g. dev build).

**Exit code:** `0`.

---

## `anchor update --yes`

### Invocation
`anchor update --yes`

**Behavior:** Same as interactive but skip the prompt. Use in scripts.

**Exit code:** `0` on success.

---

## `anchor update --channel beta`

### Invocation
`anchor update --channel beta`

**Behavior:** Query the beta channel instead of stable. Channel preference is per-invocation only, NOT persisted. To persist: `anchor settings set update.channel beta` (future key; not in 1.0).

---

## `anchor update --post-update` (hidden)

Internal flag. The new binary spawns itself with `--post-update` as the final step of the self-update flow to run any one-time migrations that apply only after a fresh binary is in place (schema migrations that require the new code, config rewrites, etc.).

Hidden from `anchor update --help`. Never documented in primary help.

### Signature
`anchor update --post-update`

### Behavior
1. Assert we're running the just-downloaded version (compare `CARGO_PKG_VERSION` against the caller's recorded expectation via a small state file at `$OOBO_HOME/.post-update-pending`).
2. Run `crate::boot::maybe_migrate_to_v1()` (or the equivalent migration entry point for the new version).
3. Delete `.post-update-pending` on success.
4. Print one line of confirmation.

**Example output:**
```
post-update migration complete: schema v6 → v7, config rewrite ok.
```

**Exit code:** `0` on success; `1` on failure (leaves `.post-update-pending` in place so the user can retry with `anchor update --post-update` manually).

### User-visible safeguard
When `.post-update-pending` is present and the user runs any other anchor command, anchor emits a one-line warning and auto-runs `--post-update`:

```
anchor: finishing update... (one-time migration)
```

After migration succeeds, the original command proceeds.

---

## `anchor update --force`

### Invocation
`anchor update --force` (when already at latest)

**Behavior:** Re-download and reinstall the current version. Useful for repairing corrupted binaries.

**Example output:**
```
forcing reinstall of anchor 1.0.0...
  downloading... ok
  verifying checksum... ok
  swapping binary... ok

reinstalled anchor 1.0.0.
```

---

## Error cases

### Network failure
`anchor update`

**Example output (stderr):**
```
error: could not reach update server: timeout after 10s
       check https://api.github.com/ and try again.
```
**Exit code:** `1`.

### Checksum mismatch
```
error: checksum verification failed for anchor-1.1.0-aarch64-darwin.tar.gz.
       the download was corrupted or tampered with. aborted, no changes made.
```
**Exit code:** `1`.

### Binary not writable (no sudo)
```
error: cannot write to /usr/local/bin/anchor (permission denied).
       retry with: sudo anchor update
       or reinstall via your package manager.
```
**Exit code:** `1`.

### Installed via package manager (brew/apt/...)
If anchor detects it was installed via a package manager (e.g. the binary resides inside a brew prefix), it refuses to self-update and points to the package manager:

```
error: anchor was installed via Homebrew.
       update via: brew upgrade anchor
```
**Exit code:** `1`.

Detection: the binary's directory matches a known package-manager prefix, or the install path contains `/Cellar/` or `/homebrew/`.

---

## Invariants

- `anchor update --check` NEVER modifies the filesystem.
- `anchor update` NEVER partially replaces the binary (always atomic: download → verify → swap).
- On failure at any step, the old binary is untouched.
- Running `anchor update` on a version ahead of latest (e.g. dev build) prints a friendly warning and exits `0` without action.
- `anchor update --yes` never waits for user input.
- The update manifest URL is fixed in 1.0 (not user-configurable). A future `update.manifest_url` settings key may be added.
