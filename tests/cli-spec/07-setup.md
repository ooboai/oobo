# `anchor setup`

The single onboarding + repair + reindex entry point. A TTY wizard that combines: project discovery, per-project enable/disable, alias install prompt, API key paste, force reindex, tool/hook re-detection, orphan-branch repair.

Flags (all optional):
- `--non-interactive` — CI-safe; accept defaults, never prompt, never open a TUI.
- `--reindex` — force full reindex of every enabled project (replaces the deleted `anchor index --force`).
- `--uninstall-alias` — remove the `git=anchor` shell alias without re-running the wizard.
- `--repair` — re-install hooks, re-detect tool paths, repair the orphan branch where needed.

Flags are composable (e.g. `--repair --reindex`), except `--non-interactive` which forces headless defaults and cannot be combined with the wizard's interactive flags.

---

## `anchor setup` — interactive wizard

### Invocation
`anchor setup`

**Context:** TTY attached.

**Behavior:** Walk through these phases sequentially:

1. **Scan:** crawl the home directory (bounded, configurable) for git repos with detectable AI sessions. Show a progress bar.
2. **Project checklist:** list every discovered project with a checkbox. Checked = enabled. Space toggles, Enter continues.
3. **Alias:** prompt `alias git=anchor? [y/N]`. On `y`, call the same code path as `anchor alias install`.
4. **Summary:** one screen showing what changed, exit.

For remote search, set an API key separately: `anchor settings set key <...>`.

**Example output (shape):**
```
anchor setup

  scanning ~ for projects...  [████████  ] 12/15 tools · 847 sessions found

  found 12 projects. sync to anchor? (space to toggle · enter to continue)
    [x] oobo-cli             (cursor, claude)    · 847 sessions
    [x] my-app               (cursor)            · 42 sessions
    [ ] work-confidential    (cursor)            · 320 sessions
    ...

  alias `git` → `anchor`? [y/N]: y

  done.
    · 10 projects enabled, 2 disabled
    · alias installed to ~/.zshrc

  you have memory.
```

**Exit code:** `0` on clean completion, `130` (SIGINT) if user Ctrl-Cs.

**Side effects:**
- `projects` table rows inserted/updated as cache/projection for each discovered project.
- `.oobo/config` created/updated for each enabled project; disabled projects keep `[project].enabled = false` if already configured.
- `~/.oobo/config.toml` updated with the API key (if provided).
- Shell RC file updated (if alias accepted).
- Git hooks installed in every enabled project.

---

## `anchor setup --non-interactive`

### Invocation
`anchor setup --non-interactive`

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

## `anchor setup --reindex`

### Invocation
`anchor setup --reindex`

**Behavior:** For every enabled project, force a FULL reindex (ignore staleness markers; reparse every tool-session path; rebuild derived tables). Shows per-project progress when TTY; emits one-line-per-project summaries in `--agent`.

**Example output (pretty):**
```
reindexing 12 projects...
  ✓ oobo-cli             847 sessions  12.4 MB  3.2s
  ✓ my-app               42 sessions   0.8 MB   0.4s
  ...

done. 12 projects · 1,892 sessions · 28 MB indexed in 14.1s.
```

**Exit code:** `0` on success; `1` if any project failed (listed on stderr).

### Invocation
`anchor setup --reindex --agent`

**Example output:**
```
reindex oobo-cli 847 12.4M 3.2s ok
reindex my-app 42 0.8M 0.4s ok
```

---

## `anchor setup --uninstall-alias`

### Invocation
`anchor setup --uninstall-alias`

**Behavior:** Same semantics as `anchor alias uninstall`. Provided here so users who think of setup as the "manage my install" command can find it.

**Example output:**
```
removed 'alias git=anchor' from ~/.zshrc
```
**Exit code:** `0`.

---

## `anchor setup --repair`

### Invocation
`anchor setup --repair`

**Behavior:** No discovery. No new prompts. For every enabled project:

- Re-install `post-commit`, `pre-push`, `post-merge`, and `post-rewrite` hooks if missing or outdated.
- Re-detect tool session paths (in case a tool was installed after initial setup).
- Run `git_check` against the orphan branch; if it's broken or missing, offer to rebuild (TTY: prompt `[Y/n]`; non-TTY: auto-rebuild).

**Example output:**
```
repairing 12 projects...
  oobo-cli            hooks ok · tools ok · orphan ok
  my-app              hooks installed · tools ok · orphan rebuilt
  work-api            hooks ok · tools ok · orphan ok

12 projects healthy.
```
**Exit code:** `0`.

### Orphan branch rebuild confirmation (TTY)

When `--repair` detects a broken orphan branch:

```
  work-api: orphan branch 'oobo/anchors/v1' is missing or corrupt.
  rebuild from local DB? [Y/n]:
```

On `y` (or Enter): rebuild from anchors stored in `oobo.db`. On `n`: skip and report.

### Non-TTY auto-behavior

With `--non-interactive` (or auto-detected non-TTY) the rebuild is executed automatically and logged.

---

## Composition

### Invocation
`anchor setup --repair --reindex --non-interactive`

**Behavior:** Non-interactive repair + full reindex. The composable pattern for CI / provisioning.

**Exit code:** `0` on success.

---

## Error cases

### TTY lost mid-wizard
If stdin closes unexpectedly during prompts: fall back to accepting defaults for remaining prompts, print a warning, exit `0`.

### `~/` not writable
Hard fail. `error: cannot create ~/.oobo/ (permission denied)`. Exit `1`.

### Conflicting flags
`anchor setup --non-interactive --uninstall-alias`

**Behavior:** Allowed; `--uninstall-alias` runs headlessly.

`anchor setup --non-interactive` combined with bare `anchor setup` itself (re-running from middle of wizard): not applicable — each invocation is independent.

---

## Invariants

- Running `anchor setup --non-interactive` twice in a row produces no additional side effects (idempotent).
- `anchor setup --repair` never deletes data. It only installs, repairs, or rebuilds FROM the local DB.
- `anchor setup --reindex` on a project the user has `anchor disable`d → SKIPPED (warning on stderr: `skipping disabled project 'foo'`).
- The wizard never scans outside `$HOME` (configurable via `anchor settings set setup.scan_roots <paths>` for advanced users).
- `anchor setup --non-interactive` produces a deterministic exit code and can be used as a first-boot step in containers / dev images.
