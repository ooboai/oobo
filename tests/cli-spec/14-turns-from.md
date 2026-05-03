# `oobo goto` / `oobo back`

Time-travel between turns and commits. Working memory (turns) is captured
into hidden Git refs and is visible through bare `oobo`; it is read-only
unless the user runs `oobo goto <id>`.

---

## `oobo goto <turn-id>`

#### Invocation
`oobo goto t123abc`

**Behavior:** Load the turn snapshot into the worktree. If the worktree has
uncommitted changes, they are automatically stashed. Records the current HEAD
so `oobo back` can return. Does not move HEAD — only updates the index and
worktree.

**Exit code:** `0` when loaded, `1` when blocked or not found.

---

## `oobo goto <commit-sha>`

#### Invocation
`oobo goto abc123`

**Behavior:** Load the commit's tree into the worktree. Auto-stashes if dirty.
Records the return point for `oobo back`.

**Exit code:** `0` when loaded, `1` when blocked or not found.

---

## `oobo back`

#### Invocation
`oobo back`

**Behavior:** Return to the state before the last `goto`. Restores the original
HEAD tree and pops the auto-stash if one was created. Refuses to run if the
worktree has changes since the goto (commit or stash them first).

**Exit code:** `0` on success, `1` if nothing to return to or worktree is dirty.

---

## `oobo goto --no-stash`

#### Invocation
`oobo goto t123abc --no-stash`

**Behavior:** Fails if the worktree has uncommitted changes instead of auto-stashing.

**Exit code:** `1` when dirty.

---

## Invariants

- Bare `oobo` is the only public memory listing.
- `oobo goto` auto-stashes by default (safe for the user).
- `oobo back` restores the previous state cleanly.
- Loading a turn or anchor does not move `HEAD`; it only updates the index and
  worktree.
- The return state is stored in `.git/oobo-state/goto-return.json`.
- Only one level of goto is tracked (a second goto overwrites the return point).
