# `oobo alias`

Install or uninstall the `alias git='oobo'` shell alias. Lets users type `git` while getting oobo's decorated behavior (commit interception, blame with AI column, passthrough for everything else).

Two subcommands, both required: `install`, `uninstall`. No positional args. No flags other than `--agent` / `--json`.

---

## `oobo alias install`

### Shell auto-detection

Detection order:

1. `$SHELL` environment variable — take basename (`bash`, `zsh`, `fish`).
2. If `$SHELL` is unset, fall back to `/etc/passwd` lookup for the current user.
3. If still unknown → exit `1` with "could not detect your shell; supported: bash, zsh, fish".

### RC file paths (per shell)

| Shell | RC file written |
|---|---|
| `bash` | `$HOME/.bashrc` (macOS also touches `$HOME/.bash_profile` if it exists) |
| `zsh`  | `$HOME/.zshrc` |
| `fish` | `$HOME/.config/fish/config.fish` |

### Block format

A delimited block that `uninstall` can find and remove precisely:

    # >>> oobo alias >>>
    alias git='oobo'
    # <<< oobo alias <<<

For fish:

    # >>> oobo alias >>>
    alias git 'oobo'
    # <<< oobo alias <<<

### Invocation: fresh install

`oobo alias install`

**Context:** shell is `zsh`, `$HOME/.zshrc` exists and contains no oobo block.

**Behavior:** Append the block to `$HOME/.zshrc` with a leading blank line if the file doesn't end with one. Print confirmation + the one-shot "restart your shell" hint.

**Example output:**
```
installed 'alias git=oobo' to ~/.zshrc
restart your shell or run: source ~/.zshrc
```
**Exit code:** `0`.

**Side effects:**
- `~/.zshrc` appended with the block (3 lines + 1 leading blank).

### Invocation: already installed

`oobo alias install` (run twice)

**Behavior:** Idempotent. Detect the existing block and exit without writing.

**Example output:**
```
alias already installed in ~/.zshrc
```
**Exit code:** `0`.

### Invocation: RC file does not exist yet

`oobo alias install`

**Context:** `$HOME/.zshrc` does not exist.

**Behavior:** Create the file with `0600` permissions and write the block.

**Example output:**
```
created ~/.zshrc with alias 'alias git=oobo'
restart your shell or run: source ~/.zshrc
```
**Exit code:** `0`.

### Invocation: RC file read-only

`oobo alias install` (RC file has `0444` perms)

**Behavior:** Hard fail. Print the exact line the user should add manually.

**Example output (stderr):**
```
error: cannot write to ~/.zshrc (read-only). add this line yourself:

    alias git='oobo'
```
**Exit code:** `1`.

### Invocation: fish shell

`oobo alias install` (with `$SHELL=/usr/local/bin/fish`)

**Behavior:** Writes fish-syntax alias to `$HOME/.config/fish/config.fish`, creating the dir if needed.

**Example output:**
```
installed 'alias git oobo' to ~/.config/fish/config.fish
restart your shell or run: source ~/.config/fish/config.fish
```
**Exit code:** `0`.

### Agent / JSON modes

`oobo alias install --agent`

**Example output:**
```
installed ~/.zshrc
```
**Exit code:** `0`.

`oobo alias install --json`

**Example output:**
```json
{ "shell": "zsh", "rc_file": "/Users/teddy/.zshrc", "status": "installed" }
```

Possible `status` values: `installed`, `already_installed`, `created_and_installed`.

---

## `oobo alias uninstall`

### Invocation: installed

`oobo alias uninstall`

**Behavior:** Remove the exact block (including the two delimiter comments and the alias line). Leave surrounding content untouched (no reformatting, no trimming of unrelated blank lines). Collapse at most one adjacent blank line.

**Example output:**
```
removed 'alias git=oobo' from ~/.zshrc
```
**Exit code:** `0`.

**Side effects:**
- `~/.zshrc` edited; content before and after the block is preserved byte-for-byte.

### Invocation: not installed

`oobo alias uninstall`

**Behavior:** No-op.

**Example output:**
```
no oobo alias found in ~/.zshrc
```
**Exit code:** `0`.

### Invocation: block present but RC file is read-only

**Example output (stderr):**
```
error: cannot write to ~/.zshrc (read-only). remove this block yourself:

    # >>> oobo alias >>>
    alias git='oobo'
    # <<< oobo alias <<<
```
**Exit code:** `1`.

### Invocation: RC file does not exist

`oobo alias uninstall`

**Example output:**
```
nothing to uninstall (~/.zshrc does not exist).
```
**Exit code:** `0`.

### Agent / JSON modes

`oobo alias uninstall --json`

**Example output:**
```json
{ "shell": "zsh", "rc_file": "/Users/teddy/.zshrc", "status": "uninstalled" }
```

Possible `status` values: `uninstalled`, `not_installed`, `rc_file_missing`.

---

## Error cases

### No subcommand
`oobo alias`

**Behavior:** Clap error.

**Example output (stderr):**
```
error: requires a subcommand: install, uninstall

Usage: oobo alias <COMMAND>
```
**Exit code:** `2`.

### Unknown subcommand
`oobo alias toggle`

**Example output (stderr):**
```
error: unrecognized subcommand 'toggle'

Usage: oobo alias <COMMAND>
```
**Exit code:** `2`.

### Unsupported shell
`oobo alias install` (with `$SHELL=/bin/tcsh`)

**Example output (stderr):**
```
error: shell 'tcsh' not supported. supported: bash, zsh, fish.
       add this line manually: alias git='oobo'
```
**Exit code:** `1`.

---

## Invariants

- `install` is idempotent: N runs produce the same file state as 1 run. The RC file is never duplicated or corrupted.
- `uninstall` precisely removes only the delimited block oobo wrote. Other `alias git=...` lines that oobo did NOT add are left alone.
- The block delimiter strings `# >>> oobo alias >>>` / `# <<< oobo alias <<<` are stable across releases (migration contract).
- For all three shells, install → uninstall → install returns the RC file to a state byte-identical to the first install.
- Neither subcommand requires a git repo; both work from anywhere.
