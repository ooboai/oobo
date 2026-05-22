# `oobo alias` — removed

The `alias` subcommand (install/uninstall of `alias git='oobo'`) was removed in 1.0. oobo is standalone — no git alias needed. Git passthrough is also removed.

Running `oobo alias` now produces a legacy hint (see `11-legacy-hints.md`).

---

## Legacy hint

### Invocation
`oobo alias`

**Example output (stderr):**
```
oobo: 'alias' was removed in 1.0.
      removed. oobo is standalone — no git alias needed.
      (this hint will be removed in 1.1.0)
```

**Exit code:** `2`.

**Side effects:** none. No RC file is touched.

---

### Invocation with subcommand args
`oobo alias install`

**Behavior:** The legacy hint system intercepts `alias` before clap sees `install`. Same hint as bare `oobo alias`.

**Example output (stderr):**
```
oobo: 'alias' was removed in 1.0.
      removed. oobo is standalone — no git alias needed.
      (this hint will be removed in 1.1.0)
```

**Exit code:** `2`.

---

## Invariants

- `oobo alias` always exits `2` with the legacy hint.
- No mapped command — the hint has no interactive "run X now?" prompt since there is no replacement.
- The hint is removed in 1.1.0+.
