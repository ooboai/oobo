# `anchor from`

Explicit loading for anchors and working memory. Working memory is captured
into hidden Git refs and is visible through `anchor anchors`; it is read-only
unless the user runs `anchor from ... --load`.

---

## `anchor from turn`

### Preview

#### Invocation
`anchor from turn t123 --agent`

**Behavior:** Preview the turn. Does not mutate the worktree.

**Exit code:** `0` when found.

### Load

#### Invocation
`anchor from turn t123 --load --agent`

**Behavior:** Load the turn snapshot into the worktree. Refuses to overwrite a
dirty worktree unless `--force` is present. The next captured turn records
`restored_from` so continuation after a load stays traceable. If captured
memory exists, anchor materializes it under the Git-local `oobo-state/from`
directory and reports the path in loaded output.

**Exit code:** `0` when loaded, `1` when blocked or missing.

---

## `anchor from anchor`

### Preview

#### Invocation
`anchor from anchor HEAD --agent`

**Behavior:** Preview the anchor/commit tree. Does not mutate the worktree.

**Exit code:** `0` when found.

### Load

#### Invocation
`anchor from anchor HEAD --load --agent`

**Behavior:** Load the anchor commit tree into the worktree. Refuses to
overwrite a dirty worktree unless `--force` is present. The next captured turn
records `restored_from=anchor:<commit>`.

**Exit code:** `0` when loaded, `1` when blocked or missing.

## Invariants

- `anchor anchors` is the only public memory listing.
- `anchor from ...` is preview-only unless `--load` is explicit.
- Dirty worktrees are protected unless `--force` is explicit.
- Loading a turn or anchor does not move `HEAD`; it only updates the index and
  worktree.
