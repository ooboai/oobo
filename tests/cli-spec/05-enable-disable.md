# `oobo enable` / `oobo disable`

Per-project toggle. Imperative verbs — NOT a settings key. The source of truth is the project folder:

- Enabled: `.oobo/config` exists and `[project].enabled` is not `false`.
- Disabled: `.oobo/config` exists with `[project].enabled = false`.
- Not enabled: `.oobo/config` does not exist.

The DB may cache indexed data, but it does not decide whether a project is enabled.

No positional args. No flags on either verb (other than the global `--agent` / `--json` / `--interactive`).

---

## `oobo enable`

### Inside an enabled repo (already on)

#### Invocation
`oobo enable`

**Behavior:** Idempotent. Print one line confirming the state; no config rewrite.

**Example output:**
```
oobo is already enabled for '$PROJECT_NAME'.
```
**Exit code:** `0`.

### Inside a disabled repo

#### Invocation
`oobo enable`

**Behavior:** Set `[project].enabled` back to `true` in `.oobo/config` (omitted on disk because it is the default). Trigger a background reindex. Print confirmation.

**Example output:**
```
oobo enabled for '$PROJECT_NAME'. indexing sessions in the background.
```
**Exit code:** `0`.

**Side effects:**
- `.oobo/config` is created or updated.
- Detached thread scans tool-session paths and enriches the DB.
- Git hooks (`post-commit`, `pre-push`, `post-merge`, `post-rewrite`) installed if not already.

### Inside a brand-new repo (never seen by oobo)

#### Invocation
`oobo enable`

**Behavior:** Create `.oobo/config` with a stable project id. Create/cache the project row if missing. Install hooks. Kick off initial index.

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

**Behavior:** Set `[project].enabled = false` in `.oobo/config`. Stop auto-indexing and commit enrichment. Leave existing anchors intact (disable is reversible, no data deletion).

**Example output:**
```
oobo disabled for '$PROJECT_NAME'. existing anchors retained. run 'oobo enable' to resume.
```
**Exit code:** `0`.

**Side effects:**
- `.oobo/config` is updated.
- Nothing deleted; nothing uninstalled. Hooks stay in place but early-exit when they see `[project].enabled = false`.

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

### Context: first `oobo` (or any view command) in a brand-new enabled repo

**Behavior:** Before the normal output, print a single one-shot banner to stderr:

**Example output (stderr, once):**
```
oobo: tracking this repo. disable: oobo disable
```

After display, record the local one-shot state. Never emit again for this project on that machine.

**Non-TTY:** silent. The banner is suppressed entirely. The agent doesn't see it, but the project is still enabled.

---

## Invariants

- `oobo enable` is idempotent: running it twice is indistinguishable from running it once in side-effect terms (no duplicate hooks, no duplicate rows).
- `oobo disable` NEVER deletes anchors or sessions.
- A disabled project retains its anchors and sessions; `oobo enable` resumes tracking.
- The banner is shown AT MOST ONCE per project per machine.
- Both verbs accept `--agent` and `--json`.
- Neither verb takes positional args; passing any → exit `2` with clap error.
