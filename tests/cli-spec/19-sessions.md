# `oobo sessions` / `oobo session`

List, inspect, share, and migrate AI sessions associated with the current repository.

---

## List sessions

### Invocation
`oobo sessions`

**Behavior:** List every session this repo knows about: locally-homed conversations, foreign-home provenance stubs (with pointer + hydration status), and live hook-capture sessions. Sorted by `updated_at` descending.

**Output (text mode):** One line per session:
```
a1b2c3d4  cursor      3t  (untitled)  home [live]
e5f6g7h8  claude      1t  (untitled)  @origin/other-repo (stub — no access)
```

Fields: uid prefix (8 chars), tool, turn count, title, home location, live marker.

`turn_count` is the max of the stored counter, the stub counter, and the readable conversation turn count — a monotonic lower bound.

**Exit code:** `0`.

---

## List sessions — JSON mode

### Invocation
`oobo sessions --json`

**Behavior:** Same data as text mode, emitted as a JSON object with `repo_id` and `sessions` array.

**Output:**
```json
{
  "repo_id": "...",
  "sessions": [
    {
      "session_uid": "...",
      "native_session_ids": ["..."],
      "tool": "cursor",
      "home_location": null,
      "hydration": { "kind": "local" },
      "turn_count": 3,
      "conversation_turns": 3,
      "updated_at": 1718100000,
      "live": true
    }
  ]
}
```

**Exit code:** `0`.

---

## List sessions — with pointer resolution

### Invocation
`oobo sessions --resolve`

**Behavior:** Same as `oobo sessions` but follows pointers and hydrates foreign-home conversations now (may involve network I/O to fetch from remote).

**Exit code:** `0`.

---

## Show one session

### Invocation
`oobo session show a1b2c3d4`

**Behavior:** Resolve and display one session's full details, following the pointer chain to hydrate the conversation when accessible. The uid argument accepts: exact uid, unambiguous uid prefix, or exact native session id.

**Output (text mode):**
```
session a1b2c3d4e5f6...
  tool: cursor  model: claude-sonnet-4-20250514
  conversation: here (home store)
  turns: 3 (3 with conversation readable here)
  repos touched: repo-id-1, repo-id-2
```

**Exit code:** `0` on success, `1` if not found.

---

## Show one session — JSON mode

### Invocation
`oobo session show --json a1b2c3d4`

**Behavior:** Same as text mode, emitted as a JSON object with `session_uid`, `hydration`, `turn_count`, `conversation_turns`, and the full `session` record.

**Exit code:** `0` on success, `1` if not found.

---

## Share a session

### Invocation
`oobo session share a1b2c3d4 --to ../other-repo`

**Behavior:** Consent-based copy of a session's conversation into another repo's v2 store. The original stays put; the copy is self-contained (homed in the target). The conversation must be locally readable (home or local checkout). Stub-only or cached-only sessions cannot be shared. Idempotent: re-sharing skips already-present turns.

**Output (text mode):**
```
shared session a1b2c3d4... into /path/to/other-repo (3 turns copied).
```

**Exit code:** `0` on success, `1` if not found or not accessible.

---

## Migrate session pointers

### Invocation
`oobo session migrate`

**Behavior:** Re-point provenance stubs after the home remote configuration changed (e.g. `.oobo/config` was edited to use a different anchor remote). For every session homed in this repo: if the stored home pointer differs from the current one, update the provenance stub's `home_location` and `updated_at`. Sessions homed elsewhere are untouched.

**Output (text mode):**
```
home is origin:other-remote; 2 session pointer(s) updated.
```

**Exit code:** `0`.

---

## Invariants

- `oobo sessions` and `oobo session show` never modify data — read-only operations.
- `oobo session share` writes only to the target repo, never the source.
- `oobo session migrate` writes only provenance stubs in the current repo.
- All subcommands exit `0` on success, `1` on user error (not found, not accessible).
- `--json` output is always a single JSON object on stdout.
- Session uid resolution accepts: exact uid, unambiguous uid prefix, or exact native session id.
