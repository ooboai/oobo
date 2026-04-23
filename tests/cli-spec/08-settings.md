# `oobo settings`

Declarative KV config. NOT imperative actions — those live in `enable`/`disable`/`alias`/`update`. Storage:

- **Default scope** → `~/.oobo/config.toml` (this user, all projects).
- **Project scope** → `project_settings` table in `~/.oobo/db/oobo.db` keyed by `project_id`.

## Grammar

Single positional grammar. No `--flags` anywhere in this command.

    oobo settings [scope] [verb] <key> [value]

- `scope` (optional, defaults to `default`): `default` | `project`.
- `verb` (optional, defaults to `get`): `set` | `unset`.
- Any other token is a `<key>` or `<value>`.
- Ambiguity resolution: reserved scope/verb words are recognized ONLY in positions 1–2 of the post-`settings` argv. After that, they're treated as key/value.

## Reserved keys

| Key | Type | Default | Meaning |
|---|---|---|---|
| `key` | string | none | API key for oobo.ai cloud (user pastes directly; no OAuth in 1.0) |
| `remote` | URL | `https://api.oobo.ai` | Remote server URL |
| `transparency` | `on`/`off` | `on` | Whether raw transcripts are stored (advanced; hidden from primary help) |
| `tools.experimental` | `on`/`off` | `off` | Opt into contrib tool adapters (Windsurf, Trae, Amp, Junie, Kiro) |
| `setup.scan_roots` | comma-list | `~` | Where `oobo setup` scans for projects (advanced) |

Unknown keys are rejected with exit code `2`. No free-form keys in 1.0.

---

## GET — show

### `oobo settings`

**Behavior:** Show ALL effective settings (defaults merged with project overrides when inside a repo). Each row indicates `default` or `project` as its source.

**Example output (pretty):**
```
key                  source     value
────                 ──────     ─────
key                  default    sk_**************************
remote               default    https://api.oobo.ai
transparency         default    on
tools.experimental   default    off
```

**Example output (`--agent`):**
```
key               default  sk_**************************
remote            default  https://api.oobo.ai
transparency      default  on
tools.experimental default off
```

**Example output (`--json`):**
```json
{
  "effective": {
    "key": { "source": "default", "value": "sk_**************************" },
    "remote": { "source": "default", "value": "https://api.oobo.ai" },
    "transparency": { "source": "default", "value": "on" },
    "tools.experimental": { "source": "default", "value": "off" }
  }
}
```

**Exit code:** `0`.

### Project-override example (inside a repo with an override)

```
key                  source     value
────                 ──────     ─────
key                  project    sk_**************************
remote               default    https://api.oobo.ai
...
```

---

### `oobo settings default`

**Behavior:** Show ONLY defaults (whether a project overrides them or not).

**Example output (pretty):**
```
key                  value
────                 ─────
key                  sk_**************************
remote               https://api.oobo.ai
transparency         on
tools.experimental   off
```

### `oobo settings project`

**Behavior:** Show ONLY the overrides set for THIS project. If no overrides → empty output + one-line note.

**Example output (no overrides):**
```
no project overrides set. showing defaults:
  run: oobo settings default
```

### `oobo settings project` outside a repo

**Example output (stderr):**
```
error: 'project' scope requires being inside a git repo.
```
**Exit code:** `1`.

---

### `oobo settings <key>`

**Behavior:** Show the default value for `<key>`. Equivalent to `oobo settings default <key>`.

`oobo settings remote`

**Example output:**
```
remote   default   https://api.oobo.ai
```

**Unknown key:**
```
error: unknown key 'foo'. valid keys: key, remote, transparency, tools.experimental, setup.scan_roots
```
**Exit code:** `2`.

### `oobo settings project <key>`

**Behavior:** Show the project override for `<key>`, or indicate that none exists.

**Example output (override exists):**
```
remote   project   https://oobo.internal.corp
```

**Example output (no override):**
```
remote   (no project override) falling back to default: https://api.oobo.ai
```
**Exit code:** `0`.

### `oobo settings default <key>`

**Behavior:** Show the default value for `<key>` (even if the project overrides it).

---

## SET — write

### `oobo settings set <key> <value>`

**Behavior:** Set the `default` value (implicit default scope). Writes to `~/.oobo/config.toml`.

`oobo settings set key sk_abc123`

**Example output:**
```
set default: key = sk_abc123
```

**Side effects:**
- `~/.oobo/config.toml` updated. File atomically rewritten (write to tempfile + rename).

### `oobo settings default set <key> <value>`

**Behavior:** Same as above, explicit scope.

### `oobo settings project set <key> <value>`

**Behavior:** Set the project override. UPSERT into `project_settings`. Requires being inside a repo.

`oobo settings project set remote https://oobo.internal.corp`

**Example output:**
```
set project ($PROJECT_NAME): remote = https://oobo.internal.corp
```

**Side effects:**
- Row in `project_settings` for `project_id` with this KV.

### `oobo settings project set <key> <value>` outside a repo

**Example output (stderr):**
```
error: 'project' scope requires being inside a git repo.
```
**Exit code:** `1`.

### Unknown key
`oobo settings set fake true`

**Example output (stderr):**
```
error: unknown key 'fake'. valid keys: key, remote, transparency, tools.experimental, setup.scan_roots
```
**Exit code:** `2`.

### Invalid value for the key
`oobo settings set transparency maybe`

**Example output (stderr):**
```
error: invalid value for 'transparency': expected 'on' or 'off', got 'maybe'
```
**Exit code:** `2`.

`oobo settings set remote "not a url"`

**Example output (stderr):**
```
error: invalid value for 'remote': expected http(s) URL, got 'not a url'
```
**Exit code:** `2`.

### Missing value
`oobo settings set key`

**Example output (stderr):**
```
error: 'set' requires a value: oobo settings [scope] set <key> <value>
```
**Exit code:** `2`.

---

## UNSET — remove

### `oobo settings unset <key>`

**Behavior:** Remove the default value for `<key>`. File re-written. If `<key>` is not present → no-op + info line (no error).

`oobo settings unset key`

**Example output:**
```
unset default: key
```

### `oobo settings default unset <key>`

**Behavior:** Same as above, explicit scope.

### `oobo settings project unset <key>`

**Behavior:** Remove the project override for `<key>`, falling back to the default. Requires being inside a repo.

`oobo settings project unset remote`

**Example output:**
```
unset project ($PROJECT_NAME): remote. falling back to default: https://api.oobo.ai
```

### Unset of a non-existent override
`oobo settings project unset remote` (no override exists)

**Example output:**
```
no project override for 'remote' to unset.
```
**Exit code:** `0`.

---

## Agent / JSON for mutations

### Invocation
`oobo settings set key sk_abc --agent`

**Example output:**
```
set default key sk_abc
```

### Invocation
`oobo settings project unset remote --json`

**Example output:**
```json
{ "action": "unset", "scope": "project", "project": "$PROJECT_NAME", "key": "remote" }
```

---

## Grammar edge cases

### Key that collides with a reserved word
Reserved words (`default`, `project`, `set`, `unset`) are only recognized in positions 1–2. A key named `set` is still invalid because it's not in the reserved-keys list, so it fails as "unknown key". In practice, none of the 1.0 keys collide with reserved words.

### Multiple values (whitespace)
`oobo settings set remote "https://oobo internal.corp"`

**Behavior:** Shell handles quoting. Multi-word values MUST be quoted by the user. Unquoted extra positional args after a required value → clap error.

### Redundant scope + verb
`oobo settings default default get key`

**Behavior:** Clap error — unknown arg `default` in position 2 (only scope or verb allowed there).

**Exit code:** `2`.

---

## Secret masking

Any key that looks like a secret (currently: exactly `key`) is masked in pretty and agent output: show only the last 4 chars or replace body with asterisks. JSON mode ALSO masks by default; to reveal, use `--json --reveal` (a rare, explicit flag on the settings command).

### Invocation
`oobo settings key`

**Example output:**
```
key   default   sk_**********abcd
```

### Invocation
`oobo settings key --json --reveal`

**Example output:**
```json
{ "key": { "source": "default", "value": "sk_abcdefghijklmnopqrstuvwxyz" } }
```

---

## Invariants

- `oobo settings set k v` then `oobo settings k` prints `v` (round-trip).
- `oobo settings project set k v` then `oobo settings k` (inside the project) prints `v` with `source = project`.
- `oobo settings unset k` on a never-set key is a no-op (exit `0`, no error).
- Unknown scope / verb / key → exit `2` with a listing of valid choices.
- The config file (`~/.oobo/config.toml`) is written atomically (tempfile + rename) to avoid partial writes.
- `key` (the secret) is ALWAYS masked unless `--reveal` is passed.
- `oobo settings project ...` outside a repo → exit `1` with a clear error.
