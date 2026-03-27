# Commands

Full command reference for `oobo`. All commands support `--agent` and `--json` output modes.

## Enriched Commit History

```bash
oobo anchors --agent                               # Compact commit log
oobo anchors --json                                # Full JSON with file attribution
oobo anchors --agent -n 20                         # Limit to N commits
oobo a --agent -n 5                                # Short alias
```

## Blame / Attribution

```bash
oobo blame src/main.rs                             # Per-line AI attribution for file at HEAD
oobo blame src/main.rs abc123                      # Attribution at a specific commit
oobo blame src/main.rs --json                      # JSON output (FileChange object)
oobo blame src/main.rs --agent                     # Compact output
```

## Sessions

```bash
oobo sessions --agent                              # Current project sessions (compact)
oobo sessions list --agent --all                   # All projects
oobo sessions list --agent --all --tool cursor -n 10 # Filter by tool, limit
oobo sessions show <session_id> --agent            # Session summary (no messages)
oobo sessions show <session_id> --json             # Full conversation + messages + stats
oobo sessions search "keyword" --all --agent       # Search by name/content
oobo sessions export <session_id> --format md      # Export as markdown
```

## Projects

```bash
oobo projects --agent                              # All tracked projects (compact)
oobo projects --json                               # Full JSON with stats
oobo projects show <name_or_path> --agent          # Project summary
```

## Stats & Analytics

```bash
oobo stats --agent                                 # Global stats (compact)
oobo stats --json                                  # Full JSON with breakdowns
oobo stats --project <name> --agent                # Per-project
oobo stats --tool cursor --agent                   # Per-tool
oobo stats --since 7d --agent                      # Time-scoped
```

## AI Development Infographic

```bash
oobo card --agent                                  # Stats summary (compact)
oobo card --json                                   # Full JSON card data
oobo card --out card.svg                           # Save SVG infographic to custom path
oobo card --format md --out card.md                # Save markdown card
```

## Data Sources

```bash
oobo sources --agent                               # Data source coverage (compact)
oobo sources --json                                # Full JSON
oobo dash --agent                                  # Configuration overview (compact)
oobo dash --json                                   # Full JSON
```

## Diagnostics

```bash
oobo inspect --agent                               # Diagnostics (compact)
oobo inspect --json                                # Full JSON
oobo inspect --fix                                 # Auto-fix common issues
oobo version --agent                               # Just the version string
oobo version --json                                # Version info as JSON
```

## Share Sessions

```bash
oobo share <session_id> --agent                    # Share + compact response
oobo share <session_id> --out chat.md              # Save redacted session as markdown
```

## Backend Sync

```bash
oobo sync                                          # Show current sync status
oobo sync on                                       # Enable auto-sync (prompts for key if needed)
oobo sync off                                      # Disable auto-sync
oobo sync --import                                 # Import anchors from orphan branch into local DB
```

Sync is **off by default**. No data is sent to any remote server unless the user explicitly runs `oobo sync on` and configures an API key (`OOBO_SECRET_KEY` env var or `api_key` via `oobo auth login`).

## Auth & Remote

These commands only apply if the user has opted into remote sync. The default remote is `api.oobo.ai` (free). Self-hosted servers are also supported.

```bash
oobo auth login --key <api_key>                    # Authenticate (free account at oobo.ai)
oobo auth logout                                   # Remove credentials
oobo auth status                                   # Show auth state + tool keys
oobo auth set-remote https://oobo.example.com      # Point to self-hosted server
```

The `OOBO_SECRET_KEY` environment variable overrides the persisted `api_key` when set.

## Maintenance

```bash
oobo scan                                          # Discover projects/sessions
oobo index                                         # Compute token counts & analytics
oobo setup                                         # Install agent hooks + git hooks
oobo update                                        # Self-update + run migrations
```
