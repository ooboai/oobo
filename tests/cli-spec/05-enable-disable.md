# `oobo enable` / `oobo disable`

Per-project toggle. Imperative verbs — NOT a settings key. Stored in the `project_settings` DB table as `oobo: on|off`.

**Default for every new repo is `on`** (opt-out model). First TTY interaction in a new repo prints a one-time banner informing the user; non-TTY runs are silent.

No positional args. No flags on either verb (other than the global `--agent` / `--json` / `--interactive`).

---

## `oobo enable`

### Inside an enabled repo (already on)

#### Invocation
`oobo enable`

**Behavior:** Idempotent. Print one line confirming the state; no DB write.

**Example output:**
```
oobo is already enabled for '$PROJECT_NAME'.
```
**Exit code:** `0`.

### Inside a disabled repo

#### Invocation
`oobo enable`

**Behavior:** UPDATE `project_settings` to `oobo = on` for this project. Trigger a background reindex. Print confirmation.

**Example output:**
```
oobo enabled for '$PROJECT_NAME'. indexing sessions in the background.
```
**Exit code:** `0`.

**Side effects:**
- DB: `project_settings.oobo = 'on'` for `project_id`.
- Detached thread scans tool-session paths and enriches the DB.
- Git hooks (`post-commit`, `post-merge`) installed if not already.

### Inside a brand-new repo (never seen by oobo)

#### Invocation
`oobo enable`

**Behavior:** Create the project row if missing (resolved by `remote_url` / `initial_commit_sha` / `primary_path`). Set `oobo = on`. Install hooks. Kick off initial index.

**Example output:**
```
oobo enabled for '$PROJECT_NAME' (new project).
```
**Exit code:** `0`.

### Agent / JSON modes

#### Invocation
`oobo enable --agent`

**Example output:**
```
enabled $PROJECT_NAME
```
**Exit code:** `0`.

#### Invocation
`oobo enable --json`

**Example output:**
```json
{ "project": { "id": "0f5c...", "name": "$PROJECT_NAME", "path": "$REPO" }, "enabled": true, "indexing": true }
```
**Exit code:** `0`.

---

## `oobo disable`

### Inside an enabled repo

#### Invocation
`oobo disable`

**Behavior:** UPDATE `project_settings` to `oobo = off`. Stop auto-indexing. Leave existing anchors intact (disable is reversible, no data deletion).

**Example output:**
```
oobo disabled for '$PROJECT_NAME'. existing anchors retained. run 'oobo enable' to resume.
```
**Exit code:** `0`.

**Side effects:**
- DB: `project_settings.oobo = 'off'`.
- Nothing deleted; nothing uninstalled. Hooks stay in place but early-exit when they see `off`.

### Inside an already-disabled repo

#### Invocation
`oobo disable`

**Behavior:** Idempotent.

**Example output:**
```
oobo is already disabled for '$PROJECT_NAME'.
```
**Exit code:** `0`.

### Agent / JSON modes

#### Invocation
`oobo disable --json`

**Example output:**
```json
{ "project": { "id": "0f5c...", "name": "$PROJECT_NAME" }, "enabled": false }
```
**Exit code:** `0`.

---

## Error cases

### Outside a git repo

#### Invocation
`oobo enable` (from `$HOME`)

**Example output (stderr):**
```
error: not a git repository. cd into a repo first, or use 'oobo setup' to manage multiple projects.
```
**Exit code:** `1`.

#### Same for `oobo disable`

### Git repo with no remote AND no commits yet

**Behavior:** Allowed. The project row is created with `remote_url = null` and `initial_commit_sha = null`; identification falls back to `primary_path`. Project will be reconciled (and stable identifiers populated) on the first commit with a remote.

**Example output:**
```
oobo enabled for '$PROJECT_NAME' (warning: no remote and no commits yet; project identity will stabilize after first commit).
```
**Exit code:** `0`.

---

## First-TTY banner

Not a command per se — a side effect of *any* oobo invocation in a new enabled repo where the banner has not yet been shown.

### Context: first `oobo anchors` (or any view command) in a brand-new enabled repo

**Behavior:** Before the normal output, print a single one-shot banner to stderr:

**Example output (stderr, once):**
```
oobo: tracking this repo. disable: oobo disable
```

After display, set `project_settings.banner_shown = 1`. Never emit again for this project.

**Non-TTY:** silent. The banner is suppressed entirely. The agent doesn't see it, but the project is still enabled.

---

## Invariants

- `oobo enable` is idempotent: running it twice is indistinguishable from running it once in side-effect terms (no duplicate hooks, no duplicate rows).
- `oobo disable` NEVER deletes anchors or sessions.
- A disabled project continues to appear in `oobo` (bare, outside repo) cross-project view, marked `(disabled)`.
- The banner is shown AT MOST ONCE per project per machine.
- Both verbs accept `--agent` and `--json`.
- Neither verb takes positional args; passing any → exit `2` with clap error.
