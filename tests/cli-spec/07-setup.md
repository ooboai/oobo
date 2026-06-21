# `oobo setup`

The single onboarding + repair entry point. A TTY wizard that combines: project discovery, per-project enable/disable, alias install prompt, API key paste, tool/hook re-detection, orphan-branch repair.

Flags (all optional):
- `--non-interactive`  --  CI-safe; accept defaults, never prompt, never open a TUI.
- `--reindex`  --  no-op; prints "no longer needed" (kept for backward compatibility).
- `--uninstall-alias`  --  remove the `git=oobo` shell alias without re-running the wizard.
- `--repair`  --  re-install hooks in the current repo, re-detect tool paths, reconcile the orphan branch.

Flags are composable, except `--non-interactive` which forces headless defaults and cannot be combined with the wizard's interactive flags.

---

## `oobo setup`  --  interactive wizard

### Invocation
`oobo setup`

**Context:** TTY attached.

**Behavior:** Walk through these phases sequentially:

1. **Scan:** crawl the home directory (bounded, configurable) for git repos with detectable AI sessions. Show a progress bar.
2. **Project checklist:** list every discovered project with a checkbox. Checked = enabled. Space toggles, Enter continues.
3. **Summary:** one screen showing what changed, exit.

For remote search, set an API key separately: `oobo settings set key <...>`.

**Example output (shape):**
```
oobo setup

  scanning ~ for projects...  [████████  ] 12/15 tools · 847 sessions found

  found 12 projects. sync to oobo? (space to toggle · enter to continue)
    [x] oobo-cli             (cursor, claude)    · 847 sessions
    [x] my-app               (cursor)            · 42 sessions
    [ ] work-confidential    (cursor)            · 320 sessions
    ...

  done.
    · 10 projects enabled, 2 disabled

  you have memory.
```

**Exit code:** `0` on clean completion, `130` (SIGINT) if user Ctrl-Cs.

**Side effects:**
- `.oobo/config` created/updated for each enabled project; disabled projects keep `[project].enabled = false` if already configured.
- `~/.oobo/config.toml` updated with the API key (if provided).
- Git hooks installed in every enabled project.

---

## `oobo setup --non-interactive`

### Invocation
`oobo setup --non-interactive`

**Behavior:** No prompts, no TUI. Accept defaults for every choice:

- All discovered projects → enabled.
- No API key prompt (use existing or skip).
- No alias install.
- Hooks installed in all enabled projects.

**Example output:**
```
scanning...
enabled 12 projects (all discovered).
hooks installed in 12 projects.
```

**Exit code:** `0`.

---

## `oobo setup --reindex`

### Invocation
`oobo setup --reindex`

**Behavior:** No-op. Prints an informational message and exits. Kept for backward compatibility so existing scripts don't break.

**Example output:**
```
--reindex is no longer needed. anchor data lives on git orphan branches and does not require reindexing.
```

**Exit code:** `0`.

---

## `oobo setup --uninstall-alias`

### Invocation
`oobo setup --uninstall-alias`

**Behavior:** Remove the `git=oobo` shell alias from the user's RC file. Detects the shell and finds the oobo-managed block in the same way the old `alias` subcommand did.

**Example output:**
```
removed 'alias git=oobo' from ~/.zshrc
```
**Exit code:** `0`.

---

## `oobo setup --repair`

### Invocation
`oobo setup --repair`

**Behavior:** No discovery. No new prompts. Operates on the current repo only:

- Re-install `post-commit`, `pre-push`, `post-merge`, and `post-rewrite` hooks if missing or outdated.
- Re-detect tool session paths (in case a tool was installed after initial setup).
- Run `git_check` against the orphan branch; if it's broken or missing, offer to reconcile (TTY: prompt `[Y/n]`; non-TTY: auto-reconcile).

**Example output:**
```
repairing oobo-cli...
  hooks ok · tools ok · orphan ok

healthy.
```
**Exit code:** `0`.

### Orphan branch reconcile confirmation (TTY)

When `--repair` detects a broken orphan branch:

```
  orphan branch 'oobo/anchors/v2' is missing or corrupt.
  reconcile? [Y/n]:
```

On `y` (or Enter): reconcile the orphan branch. On `n`: skip and report.

### Non-TTY auto-behavior

With `--non-interactive` (or auto-detected non-TTY) the rebuild is executed automatically and logged.

---

## Composition

### Invocation
`oobo setup --repair --non-interactive`

**Behavior:** Non-interactive repair. The composable pattern for CI / provisioning.

**Exit code:** `0` on success.

---

## Error cases

### TTY lost mid-wizard
If stdin closes unexpectedly during prompts: fall back to accepting defaults for remaining prompts, print a warning, exit `0`.

### `~/` not writable
Hard fail. `error: cannot create ~/.oobo/ (permission denied)`. Exit `1`.

### Conflicting flags
`oobo setup --non-interactive --uninstall-alias`

**Behavior:** Allowed; `--uninstall-alias` runs headlessly.

`oobo setup --non-interactive` combined with bare `oobo setup` itself (re-running from middle of wizard): not applicable  --  each invocation is independent.

---

## Invariants

- Running `oobo setup --non-interactive` twice in a row produces no additional side effects (idempotent).
- `oobo setup --repair` never deletes data. It only installs, repairs, or reconciles the orphan branch.
- `oobo setup --reindex` is a no-op that prints an informational message.
- The wizard never scans outside `$HOME` (configurable via `oobo settings set setup.scan_roots <paths>` for advanced users).
- `oobo setup --non-interactive` produces a deterministic exit code and can be used as a first-boot step in containers / dev images.
