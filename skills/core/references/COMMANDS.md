# Commands

Current command reference for `oobo` 1.1.2. Commands support global `--agent`, `--json`, and `--interactive` output-mode flags. Filter flags (`-n`, `--since`, `--tool`) are also global.

## Memory Feed (bare command)

```bash
oobo --agent                                       # Compact enriched commit log
oobo --json                                        # Structured anchor summaries
oobo -n 20 --since 7d --tool cursor                # Filter anchors
oobo --agent -n 5                                  # Short and sweet
```

Equivalent to `oobo anchors`. Shows a scrollable TUI in interactive mode, or a flat list in agent/JSON mode.

## Anchor Show - drill into a commit

```bash
oobo anchor show <sha> --agent                     # Compact anchor detail
oobo anchor show <sha> --json                      # Full anchor detail
```

## Blame / Attribution

```bash
oobo blame src/main.rs                             # Git blame with AI attribution
oobo blame src/main.rs abc123                       # Blame at a specific commit
oobo blame src/main.rs --json                       # Structured per-line output
```

## Time Travel - Goto / Back

```bash
oobo goto <turn-id-or-commit-sha>                  # Travel to a turn or commit
oobo goto <id> --no-stash                          # Fail if worktree is dirty
oobo back                                          # Return to where you were
```

`goto` auto-stashes dirty changes, loads the target tree, and records a return
point. `back` restores the original HEAD and pops the stash. `goto` is strictly
repo-local: it never touches another repo's worktree.

## Sessions - cross-repo session listing and resolution

```bash
oobo sessions --agent                              # Sessions this repo knows about
oobo sessions --json                               # Pointer + hydration per session
oobo sessions --resolve                            # Follow pointers (may fetch)
oobo session show <uid> --json                     # Resolve one session fully
oobo session share <uid> --to <repo>               # Copy a conversation into another repo
oobo session migrate                               # Re-point stubs after a remote change
```

A session's conversation is stored exactly once, in its home store. Repos it
edited from elsewhere hold pointer stubs; `hydration` in the output says
whether the conversation is readable from here (`local`, `local_repo`,
`fetched`, `cached`, `stub_only`, or `live` for in-progress sessions).

## Search - semantic code search

```bash
oobo search "auth middleware" --agent               # Code search in current repo
oobo search "parse config" -k 10 --agent            # Top 10 results
oobo search "deployment" --content docs --agent     # Search docs only
oobo search "auth" --mode bm25 --agent              # Keyword only (fastest)
```

Search uses hybrid BM25 + vector search. The index is built and cached automatically on first run.

## Recall - find sessions and anchors

```bash
oobo recall "auth bug" --agent                     # Search current project
oobo recall "auth bug" --global --agent            # Search all projects
oobo recall "auth" --since 7d --tool claude        # Filter by time/tool
oobo recall "auth" --project oobo-cli --json       # Explicit project scope
```

Recall is local-first. With an API key, default recall merges local and remote results; use `--local`, `--remote`, or `--both` to force a source.

## Delta - compare two anchors

```bash
oobo delta                                         # Compare HEAD to its previous anchor
oobo delta abc123                                  # Compare a specific anchor to its previous
oobo delta abc123 def789                           # Explicit pair
oobo delta --full                                  # Include sessions, decisions, techniques
oobo delta abc123 --full --json                    # Full structured output
```

Shows what changed between two anchors: category shifts, complexity changes, new areas, new techniques, and a narrative summary. Requires an API key.

## Project Tracking

```bash
oobo enable                                        # Enable oobo in the current repo
oobo disable                                       # Disable oobo in the current repo
oobo                                               # Anchors TUI (must be inside an enabled repo)
```

Disabled projects are recorded in `.oobo/config` with `[project].enabled = false`; git hooks and capture stay quiet.

## Setup / Maintenance

```bash
oobo setup                                         # Onboard, select projects, install hooks
oobo setup --non-interactive                       # CI-safe defaults
oobo setup --reindex                               # Legacy (prints info message only)
oobo setup --repair                                # Reinstall hooks + repair local metadata
oobo update                                        # Self-update
oobo update --check                                # Check only
```

Interactive setup discovers git repos and lets users choose which to enable.

## Settings / Remote

```bash
oobo settings                                      # Show effective settings
oobo settings set key <api_key>                    # Store remote API key (global)
oobo settings project set key <api_key>            # Store API key for this project only
oobo settings unset key                            # Remove persisted API key
oobo settings project unset key                    # Remove project-level key
oobo settings set api_url https://oobo.example.com # Point to self-hosted server
oobo settings set transparency on                  # Enable transcript sync (default: on)
oobo settings set tools.experimental on            # Enable experimental tool detection
oobo settings set setup.scan_roots ~/dev,~/work    # Directories to scan for repos
oobo settings project set remote oobo              # Push anchor branch to specific remote
```

Valid keys: `key`, `api_url`, `remote`, `transparency`, `tools.experimental`, `setup.scan_roots`.

**Secrets handling:** API keys are stored in `.oobo/secrets` (gitignored, 0600 permissions), never in `.oobo/config`. This prevents accidental exposure in committed project configs. The resolution order is: `OOBO_SECRET_KEY` env var > project `.oobo/secrets` > global `~/.oobo/config`.

## MCP (Model Context Protocol)

```bash
oobo mcp                                           # Start stdio JSON-RPC server
oobo mcp install                                   # Auto-detect and configure Cursor/Claude/Copilot
oobo mcp install cursor                            # Configure a specific tool
oobo mcp install --hosted                          # Cloud-only mode (no local binary needed)
oobo mcp install --remove                          # Uninstall MCP configuration
```

The MCP server exposes these tools to AI agents:

| Tool | Description |
|------|-------------|
| `search` | Semantic code search (local, hybrid BM25 + vector) |
| `find_related` | Find code related to a query across the indexed codebase |
| `recall` | Search engineering memory (sessions, decisions, patterns) |
| `get_context` | File-scoped context from engineering history (plain-text guidance) |
| `ask` | Natural language questions against the team's engineering memory |

The server reads the project API key from `.oobo/secrets` automatically. For per-project keys in multi-project setups, use `oobo settings project set key <key>` in each project root.

## Help

```bash
oobo help                                          # List all help topics
oobo help anchors                                  # What anchors are and how they work
oobo help search                                   # Code search usage
oobo help recall                                   # Recall syntax and cloud configuration
oobo help blame                                    # Reading the AI attribution overlay
oobo help hooks                                    # Git and agent hooks explained
oobo help config                                   # All settings explained
oobo help keyboard                                 # TUI keybindings reference
```

Built-in documentation, always available offline. Works in all output modes.

## Version

```bash
oobo --version                                     # Print version
```
