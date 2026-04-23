# Git passthrough

`oobo` is designed to be a transparent decorator around `git`. When you `alias git=oobo`, every git command you type still works. Commands that are reserved oobo verbs get oobo's enhanced behavior; everything else is forwarded verbatim to the real `git` binary.

## Reserved oobo verbs (NOT forwarded)

These are the only positions where oobo intercepts:

- `anchors`, `blame`, `search`, `enable`, `disable`, `alias`, `setup`, `settings`, `update`, `hooks` (hidden).
- Global flags at the root: `--agent`, `--json`, `--interactive`, `--help`, `-h`, `--version`, `-V`.

Everything else, including **every real `git` subcommand**, is treated as passthrough.

## How passthrough works

1. Parse argv. If the first non-flag token matches a reserved oobo verb → run oobo's handler.
2. Else → locate the real `git` binary (PATH search, excluding symlinks pointing back to `oobo`), execve it with the full original argv (minus `oobo` itself).
3. Exit codes, stdout, stderr flow through unchanged.

## Examples

### Forwarded to git

`oobo status`

**Behavior:** Execs `git status` unchanged. Output and exit code are git's.

**Example output:**
```
On branch main
Your branch is up to date with 'origin/main'.

nothing to commit, working tree clean
```
**Exit code:** `0`.

---

`oobo log --oneline -n 5`

**Behavior:** `git log --oneline -n 5`.

**Example output:**
```
a1b2c3d fix auth middleware
d4e5f6g add rate limiter
7a8b9c0 wip
e1f2d3c extract payment adapter
f7a8b9c bump deps
```
**Exit code:** `0`.

---

`oobo push`

**Behavior:** `git push`. oobo's post-push hook (installed at setup time) fires after the push completes; the push exit code is unchanged.

---

`oobo diff HEAD~1`

**Behavior:** `git diff HEAD~1`. Paged through oobo's terminal unchanged.

---

`oobo commit -m "wip"`

**Behavior:** `git commit -m "wip"`. The installed `post-commit` hook runs afterward, linking AI sessions + creating the anchor. The commit's exit code is unchanged.

**Side effects:** after commit succeeds:
- Anchor row inserted into DB.
- Orphan branch updated via detached thread.
- Token usage recorded.

---

`oobo rebase -i HEAD~3`

**Behavior:** Fully interactive git rebase — oobo does nothing special, git's editor opens as usual. Post-rebase, the `post-rewrite` hook (installed at setup) updates any anchors affected by the rewrite.

---

`oobo nonexistent-verb`

**Behavior:** Forwarded to git, which returns its usual `git: 'nonexistent-verb' is not a git command.` error.

**Example output (stderr):**
```
git: 'nonexistent-verb' is not a git command. See 'git --help'.
```
**Exit code:** `1`.

---

## Intercepted verb: `blame`

### Invocation
`oobo blame src/main.rs`

**Behavior:** oobo's enhanced blame (strict superset of git's; see `03-blame.md`). NOT forwarded.

To get pure git blame output, use `oobo blame src/main.rs --no-ai` (byte-identical to `git blame`).

### Invocation (alias active, user types `git blame`)
`git blame src/main.rs`

**Behavior:** When `alias git=oobo` is in effect, this invokes oobo, which runs its enhanced blame. Users who want pure git blame can use `\git blame` (bypass alias) or `oobo blame --no-ai`.

---

## Invariants

- For every `<verb>` that is NOT a reserved oobo verb, `oobo <verb> <args>` has the same stdout, stderr, and exit code as `git <verb> <args>`.
- oobo NEVER modifies the forwarded argv (no filtering, no reordering, no injected flags).
- oobo NEVER injects flags into git.
- When oobo cannot locate a real `git` binary, it errors with: `fatal: git not found on PATH. install git and retry.` and exit `127`.
- Cycle protection: oobo resolves git via PATH but skips any entry that, after symlink resolution, points to the oobo binary itself. Prevents `alias git=oobo` infinite recursion.

## Git-not-installed error

### Invocation
`oobo status` (git not installed)

**Example output (stderr):**
```
fatal: git not found on PATH. install git and retry.
```
**Exit code:** `127`.

## Infinite-recursion guard

### Scenario
Someone symlinks `/usr/local/bin/git` → `/usr/local/bin/oobo`, and invokes `oobo status`.

**Behavior:** oobo walks PATH, reads each `git` candidate with `realpath`, skips any that point back to itself. If NO candidates remain, emit the "git not found" error rather than infinite-recursing.

**Exit code:** `127`.
