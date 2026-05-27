# `oobo help`

Built-in documentation system. Shows rich prose-based help for specific topics. Always available, works offline, respects output modes.

Positional arg:
- `<topic>`  --  optional. Omit for a list of all topics.

Available topics:
- `anchors`  --  What anchors are and how they work
- `search`  --  Search syntax, filters, and cloud vs local
- `blame`  --  Reading the AI attribution overlay
- `hooks`  --  How git and agent hooks capture sessions
- `config`  --  All settings explained
- `keyboard`  --  TUI keybindings reference

---

## List all topics (no argument)

### Invocation
`oobo help`

**Behavior:** Print the list of available topics with their descriptions.

**Example output (TTY):**
```
oobo help  --  built-in documentation

  Usage: oobo help <topic>

  anchors      What are anchors and how do they work
  search       Search syntax, filters, and cloud vs local
  blame        Reading the AI attribution overlay
  hooks        How git and agent hooks capture sessions
  config       All settings explained
  keyboard     TUI keybindings reference
```

**Exit code:** `0`.

---

### Invocation (agent mode)
`oobo help --agent`

**Example output:**
```
anchors What are anchors and how do they work
search Search syntax, filters, and cloud vs local
blame Reading the AI attribution overlay
hooks How git and agent hooks capture sessions
config All settings explained
keyboard TUI keybindings reference
```

**Exit code:** `0`.

---

### Invocation (JSON mode)
`oobo help --json`

**Example output:**
```json
{
  "topics": [
    { "topic": "anchors", "description": "What are anchors and how do they work" },
    { "topic": "search", "description": "Search syntax, filters, and cloud vs local" },
    { "topic": "blame", "description": "Reading the AI attribution overlay" },
    { "topic": "hooks", "description": "How git and agent hooks capture sessions" },
    { "topic": "config", "description": "All settings explained" },
    { "topic": "keyboard", "description": "TUI keybindings reference" }
  ]
}
```

**Exit code:** `0`.

---

## Show a specific topic

### Invocation
`oobo help anchors`

**Behavior:** Print the full help content for the "anchors" topic.

**Example output (TTY, shape):**
```
oobo help anchors

Anchors are the core unit of oobo memory. Every git commit that was made
while an AI coding tool was active gets an anchor  --  a metadata record that
links the commit to the AI session(s) that contributed to it.
...
```

**Exit code:** `0`.

---

### Invocation (agent mode)
`oobo help blame --agent`

**Behavior:** Print the raw prose without the bold header or ANSI formatting.

**Exit code:** `0`.

---

### Invocation (JSON mode)
`oobo help config --json`

**Example output:**
```json
{
  "topic": "config",
  "content": "oobo settings are layered: defaults apply globally, project overrides\ntake precedence when inside a repo with .oobo/config.\n..."
}
```

**Exit code:** `0`.

---

## Unknown topic

### Invocation
`oobo help nonexistent`

**Behavior:** Print error on stderr with the list of valid topics.

**Example output (stderr):**
```
error: unknown topic 'nonexistent'

available topics:
  anchors      What are anchors and how do they work
  search       Search syntax, filters, and cloud vs local
  blame        Reading the AI attribution overlay
  hooks        How git and agent hooks capture sessions
  config       All settings explained
  keyboard     TUI keybindings reference
```

**Exit code:** `2`.

---

## Works anywhere

### Invocation
`oobo help anchors` (outside any git repository)

**Behavior:** Help is self-contained  --  no repo context needed. Works the same as inside a repo.

**Exit code:** `0`.

---

## Invariants

- `oobo help` works outside a git repo (no repo context needed).
- `--agent` output never contains ANSI escapes.
- `--json` output always parses per `jq '.'`.
- Unknown topics exit `2` with a helpful error listing all valid topics.
- The topic list in help output MUST match the compiled `TOPICS` constant  --  any mismatch is a bug.
- Help content is compiled into the binary  --  always available offline, always current with the installed version.
