# Commands

Current command reference for `anchor` 1.0. Commands support global `--agent`, `--json`, and `--interactive` output-mode flags unless noted.

Any command not listed here is forwarded to git unchanged.

## Commit Memory

```bash
anchor anchors --agent                               # Compact enriched commit log
anchor anchors --json                                # Structured anchor summaries
anchor anchors -n 20 --since 7d --tool cursor        # Filter anchors
anchor a --agent -n 5                                # Short alias for anchors
anchor anchors show <sha> --json                     # Full anchor detail
```

## Blame / Attribution

```bash
anchor blame src/main.rs                             # Git blame with AI attribution
anchor blame src/main.rs abc123                      # Blame at a specific commit
anchor blame src/main.rs --no-ai                     # Pure git blame
anchor blame src/main.rs --json                      # Structured per-line output
```

## Continue / Handoff

```bash
anchor from turn <id>                              # Preview working-memory snapshot
anchor from turn <id> --load                       # Load snapshot into worktree
anchor from anchor <sha>                           # Preview committed anchor tree
anchor from anchor <sha> --load                    # Load anchor tree into worktree
```

Loads are preview-first and refuse dirty worktrees unless `--force` is explicit.

## Search

```bash
anchor search "auth bug" --agent                     # Search current project
anchor search "auth bug" --global --agent            # Search all projects
anchor search "auth" --since 7d --tool claude        # Filter by time/tool
anchor search "auth" --project oobo-cli --json       # Explicit project scope
```

Search is local-first. With an API key, default search merges local and remote results; use `--local`, `--remote`, or `--both` to force a source.

## Project Tracking

```bash
anchor enable                                        # Enable anchor in the current repo
anchor disable                                       # Disable anchor in the current repo
anchor                                               # In repo: anchor TUI; outside repo: project picker
```

Disabled projects are recorded in `.oobo/config` with `[project].enabled = false`; git hooks, background indexing, and capture paths stay quiet there.

## Setup / Maintenance

```bash
anchor setup                                         # Onboard, select projects, install hooks
anchor setup --non-interactive                       # CI-safe defaults
anchor setup --reindex                               # Force reindex of enabled projects
anchor setup --repair                                # Reinstall hooks + repair local metadata
anchor setup --uninstall-alias                       # Remove git=anchor shell alias
anchor update                                        # Self-update
anchor update --check                                # Check only
```

Interactive setup lets users choose which scanned projects anchor should track.

## Settings / Remote

```bash
anchor settings                                      # Show effective settings
anchor settings set key <api_key>                    # Store remote API key
anchor settings unset key                            # Remove persisted API key
anchor settings set remote https://oobo.example.com  # Point to self-hosted server
anchor settings set transparency on                  # Enable redacted transcript sync
anchor settings set setup.scan_roots ~/dev,~/work    # Configure setup scan roots
```

A non-empty default API key is used for remote search. `anchor settings unset key` removes the persisted key. `OOBO_SECRET_KEY` overrides the persisted key for the current process only. There is no cloud upload pipeline; team sync is Git-first via the orphan branch.

## Alias

```bash
anchor alias install                                 # Add alias git=anchor to shell rc
anchor alias uninstall                               # Remove the alias
```

## Version

```bash
anchor --version                                     # Print version
```
