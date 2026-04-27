# Global flags

Flags that apply to every subcommand (and to bare `anchor`). Parsed at the clap root with `global = true`. Position-independent: `anchor --agent anchors`, `anchor anchors --agent`, and `anchor anchors --agent -n 5` are all equivalent.

---

## `--agent`

Forces **agent mode**: ruthlessly minimal plain-text output, one item per line, no colors, no borders, no prose. Inspired by `git log --oneline`. Exists to NOT waste tokens when an LLM consumes anchor's output.

### Invocation
`anchor anchors --agent`

**Context:** inside a repo with at least one anchor.

**Behavior:** Emit a fixed-column listing, most recent first. No headers, no totals, no section dividers. Columns separated by one or more spaces; never by a tab, never by ANSI escapes.

**Example output:**
```
a1b2c3d 2m   fix auth middleware        claude 12k 1s
d4e5f6g 18m  add rate limiter           gemini 31k 2s
7a8b9c0 1h   wip                        -      -   -
```

**Exit code:** `0`.

**Side effects:** none.

---

## `--json`

Forces **JSON mode**: full-fidelity structured data for tools/scripts parsing anchor's output. Verbose by design — do NOT use for LLM consumption.

### Invocation
`anchor anchors --json`

**Context:** inside a repo with at least one anchor.

**Behavior:** Emit a single JSON document (not JSONL) to stdout. Schema documented per-command in that command's spec file. No trailing newline beyond the one at the end of the document.

**Example output:**
```json
{
  "project": { "id": "0f5c...", "path": "$REPO", "remote": "git@github.com:me/repo.git" },
  "anchors": [
    {
      "sha": "a1b2c3d",
      "timestamp": "{timestamp}",
      "subject": "fix auth middleware",
      "tools": ["claude"],
      "tokens": 12000,
      "sessions": [ { "id": "{uuid}", "intent": "the auth middleware drops..." } ]
    }
  ]
}
```

**Exit code:** `0`.

**Side effects:** none.

---

## `--agent` + `--json` combined

### Invocation
`anchor anchors --agent --json`

**Behavior:** Fail fast. The two flags have different intents (token-efficiency vs. structured fidelity) and cannot be combined.

**Example output (stderr):**
```
error: the argument '--agent' cannot be used with '--json'
```

**Exit code:** `2`.

**Side effects:** none.

---

## `--interactive`

Escape hatch. Forces pretty/TUI mode even when auto-detection would flip to `--agent` (non-TTY stdout, agent env var present). Useful when running `anchor` inside tmux/screen with piped stdout for logging but you still want the TUI on stderr. Rarely needed by end users.

### Invocation
`anchor --interactive > /tmp/out.txt`

**Context:** inside a repo.

**Behavior:** Opens the anchor-feed TUI on the terminal despite stdout being redirected. The redirect receives nothing (TUI writes directly to the terminal device).

**Exit code:** `0` (on clean `q` quit).

---

## `--help` / `-h`

### Invocation
`anchor --help`

**Behavior:** Print the top-level help. Commands are grouped by mental-model category: Views, Actions, Wizard/Config, Lifecycle.

**Example output (shape):**
```
anchor — git with memory.

USAGE:
    anchor [OPTIONS] [COMMAND]

VIEWS:
    anchors, a See the memory
    blame      Per-line AI/human attribution
    search     Find any past session

ACTIONS:
    from       Load code/context from a turn or anchor
    enable     Start tracking this project
    disable    Stop tracking this project
    alias      Install/uninstall the git=anchor shell alias

WIZARD + CONFIG:
    setup      Onboard, repair, reindex, manage projects
    settings   Show / set / unset config values

LIFECYCLE:
    update     Self-update

GIT PASSTHROUGH:
    Any command not listed above is forwarded to git unchanged.
    Write operations (commit, push, merge) also capture AI context.

OPTIONS:
    --agent          Minimal plain-text output (token-efficient)
    --json           Full structured JSON output
    --interactive    Force TUI even when auto-detection would not
    -h, --help       Print help
    -V, --version    Print version

Run `anchor <command> --help` for per-command help.
```

**Exit code:** `0`.

### Subcommand help

`anchor anchors --help`, `anchor settings --help`, etc. — every subcommand must print its own help showing ONLY its own flags and positional args. No clap-default brag about global flags except for a brief footer:

```
Global flags: --agent, --json, --interactive. See `anchor --help`.
```

---

## `--version` / `-V`

### Invocation
`anchor --version`

**Behavior:** Print one line: `anchor <semver>`. Nothing else.

**Example output:**
```
anchor 1.0.0
```

**Exit code:** `0`.

### Machine-readable variant
`anchor --version --json`

**Example output:**
```json
{ "name": "anchor", "version": "1.0.0", "commit": "{hash}", "built_at": "{timestamp}" }
```

**Exit code:** `0`.

---

## Auto-detection of agent mode

`anchor` implicitly flips to `--agent` when ANY of these is true:

- `stdout` is not a TTY (pipe, redirect).
- Any of these env vars is set and non-empty: `CURSOR_AGENT`, `CLAUDECODE`, `AIDER`, `CONTINUE_SESSION`, `CONTINUE_IDE`, `AICOMMITS`.

### Invocation
`anchor anchors | head -5`

**Behavior:** Even though no explicit `--agent` flag is passed, stdout is a pipe, so anchor emits agent-mode output. No colors, no TUI.

**Example output:**
```
a1b2c3d 2m   fix auth middleware        claude 12k 1s
d4e5f6g 18m  add rate limiter           gemini 31k 2s
7a8b9c0 1h   wip                        -      -   -
e1f2d3c 3h   extract payment adapter    cursor 28k 1s
f7a8b9c 4h   bump deps                  -      -   -
```

**Exit code:** `0`.

### Invocation
`CURSOR_AGENT=1 anchor anchors`

**Behavior:** Env var triggers agent mode even with a TTY attached. An LLM running inside Cursor's agent gets token-efficient output without having to know about the flag.

**Exit code:** `0`.

### Invocation
`anchor anchors` (plain TTY, no env vars)

**Behavior:** TTY detected, no env var, no flag → pretty mode (colored table).

**Exit code:** `0`.

---

## Negative: unknown flag

### Invocation
`anchor anchors --fake`

**Behavior:** Clap prints the error on stderr.

**Example output (stderr):**
```
error: unexpected argument '--fake' found

Usage: anchor anchors [OPTIONS]

For more information, try '--help'.
```

**Exit code:** `2`.

---

## Invariant: position independence

For every subcommand and every global flag, these must produce identical output:

- `anchor <flag> <cmd> <args>`
- `anchor <cmd> <flag> <args>`
- `anchor <cmd> <args> <flag>`

Test matrix is exercised in every subcommand's spec.
