# `anchor enable` / `anchor disable`

Per-project toggle. Imperative verbs — NOT a settings key. The source of truth is the project folder:

- Enabled: `.oobo/config` exists and `[project].enabled` is not `false`.
- Disabled: `.oobo/config` exists with `[project].enabled = false`.
- Not enabled: `.oobo/config` does not exist.

The DB may cache indexed data, but it does not decide whether a project is enabled.

No positional args. No flags on either verb (other than the global `--agent` / `--json` / `--interactive`).

---

## `anchor enable`

### Inside an enabled repo (already on)

#### Invocation
`anchor enable`

**Behavior:** Idempotent. Print one line confirming the state; no config rewrite.

**Example output:**
```
anchor is already enabled for '$PROJECT_NAME'.
```
**Exit code:** `0`.

### Inside a disabled repo

#### Invocation
`anchor enable`

**Behavior:** Set `[project].enabled` back to `true` in `.oobo/config` (omitted on disk because it is the default). Trigger a background reindex. Print confirmation.

**Example output:**
```
anchor enabled for '$PROJECT_NAME'. indexing sessions in the background.
```
**Exit code:** `0`.

**Side effects:**
- `.oobo/config` is created or updated.
- Detached thread scans tool-session paths and enriches the DB.
- Git hooks (`post-commit`, `pre-push`, `post-merge`, `post-rewrite`) installed if not already.

### Inside a brand-new repo (never seen by anchor)

#### Invocation
`anchor enable`

**Behavior:** Create `.oobo/config` with a stable project id. Create/cache the project row if missing. Install hooks. Kick off initial index.

**Example output:**
```
anchor enabled for '$PROJECT_NAME' (new project).
```
**Exit code:** `0`.

### Agent / JSON modes

#### Invocation
`anchor enable --agent`

**Example output:**
```
enabled $PROJECT_NAME
```
**Exit code:** `0`.

#### Invocation
`anchor enable --json`

**Example output:**
```json
{ "project": { "id": "0f5c...", "name": "$PROJECT_NAME", "path": "$REPO" }, "enabled": true, "indexing": true }
```
**Exit code:** `0`.

---

## `anchor disable`

### Inside an enabled repo

#### Invocation
`anchor disable`

**Behavior:** Set `[project].enabled = false` in `.oobo/config`. Stop auto-indexing and commit enrichment. Leave existing anchors intact (disable is reversible, no data deletion).

**Example output:**
```
anchor disabled for '$PROJECT_NAME'. existing anchors retained. run 'anchor enable' to resume.
```
**Exit code:** `0`.

**Side effects:**
- `.oobo/config` is updated.
- Nothing deleted; nothing uninstalled. Hooks stay in place but early-exit when they see `[project].enabled = false`.

### Inside an already-disabled repo

#### Invocation
`anchor disable`

**Behavior:** Idempotent.

**Example output:**
```
anchor is already disabled for '$PROJECT_NAME'.
```
**Exit code:** `0`.

### Agent / JSON modes

#### Invocation
`anchor disable --json`

**Example output:**
```json
{ "project": { "id": "0f5c...", "name": "$PROJECT_NAME" }, "enabled": false }
```
**Exit code:** `0`.

---

## Error cases

### Outside a git repo

#### Invocation
`anchor enable` (from `$HOME`)

**Example output (stderr):**
```
error: not a git repository. cd into a repo first, or use 'anchor setup' to manage multiple projects.
```
**Exit code:** `1`.

#### Same for `anchor disable`

### Git repo with no remote AND no commits yet

**Behavior:** Allowed. The project row is created with `remote_url = null` and `initial_commit_sha = null`; identification falls back to `primary_path`. Project will be reconciled (and stable identifiers populated) on the first commit with a remote.

**Example output:**
```
anchor enabled for '$PROJECT_NAME' (warning: no remote and no commits yet; project identity will stabilize after first commit).
```
**Exit code:** `0`.

---

## First-TTY banner

Not a command per se — a side effect of *any* anchor invocation in a new enabled repo where the banner has not yet been shown.

### Context: first `anchor anchors` (or any view command) in a brand-new enabled repo

**Behavior:** Before the normal output, print a single one-shot banner to stderr:

**Example output (stderr, once):**
```
anchor: tracking this repo. disable: anchor disable
```

After display, record the local one-shot state. Never emit again for this project on that machine.

**Non-TTY:** silent. The banner is suppressed entirely. The agent doesn't see it, but the project is still enabled.

---

## Invariants

- `anchor enable` is idempotent: running it twice is indistinguishable from running it once in side-effect terms (no duplicate hooks, no duplicate rows).
- `anchor disable` NEVER deletes anchors or sessions.
- A disabled project continues to appear in `anchor` (bare, outside repo) cross-project view, marked disabled.
- The banner is shown AT MOST ONCE per project per machine.
- Both verbs accept `--agent` and `--json`.
- Neither verb takes positional args; passing any → exit `2` with clap error.
