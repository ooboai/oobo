---
name: oobo
description: AI development control plane. Decorates git to enrich commits with session/agent metadata, tracks token usage, code attribution, and session history across Cursor, Claude Code, Gemini CLI, Codex, OpenCode, Copilot, Windsurf, Aider, Zed, and Trae.
metadata:
  author: oobo
  version: "0.1.0"
install:
  check: command -v oobo
  run: curl -fsSL https://oobo.ai/install.sh | bash -s -- --agent
---

# Oobo — AI Development Control Plane

You have access to `oobo`, a local CLI that decorates git to automatically enrich commits with AI session metadata, tracks tool usage, token counts, code attribution, and session history. When aliased as `git`, all standard git operations pass through transparently while commits get enriched with anchor metadata.

If `oobo` is not installed, run: `curl -fsSL https://oobo.ai/install.sh | bash -s -- --agent`
This returns JSON: `{"status":"ok","version":"...","binary":"...","platform":"..."}`

## The --agent flag

All commands accept `--agent` for structured JSON output. Always use `--agent` instead of `--json`.

```bash
oobo <command> --agent    # Forces JSON output on any command
```

## Commands

### Enriched Commit History

```bash
oobo anchors --agent                               # Enriched commit log as JSON
oobo anchors --agent -n 20                         # Limit to N commits
oobo a --agent -n 5                                # Short alias
```

### Sessions

```bash
oobo sessions --agent                              # Current project sessions as JSON
oobo sessions list --agent --all                   # All projects
oobo sessions list --agent --all --tool cursor -n 10 # Filter by tool, limit
oobo sessions show <session_id> --agent            # Full conversation + stats
oobo sessions search "keyword" --all --agent       # Search by name/content
oobo sessions export <session_id> --format md      # Export as markdown
```

### Projects

```bash
oobo projects --agent                              # All tracked projects as JSON
oobo projects show <name_or_path> --agent          # Project details + sessions
```

### Stats & Analytics

```bash
oobo stats --agent                                 # Global stats as JSON
oobo stats --project <name> --agent                # Per-project
oobo stats --tool cursor --agent                   # Per-tool
oobo stats --since 7d --agent                      # Time-scoped
```

### AI Development Infographic

```bash
oobo card --agent                                  # Stats as JSON (includes SVG)
oobo card --out card.svg                           # Save SVG infographic to custom path
oobo card --format md --out card.md                # Save markdown card
oobo card --format json                            # JSON output
```

### Data Sources

```bash
oobo sources --agent                               # Data source coverage as JSON
oobo dash --agent                                  # Configuration overview as JSON
```

### Diagnostics

```bash
oobo inspect --agent                               # Diagnostics as JSON
oobo inspect --fix                                 # Auto-fix common issues
oobo version --agent                               # Version info as JSON
```

### Share Sessions

```bash
oobo share <session_id> --agent                    # Redacted session as JSON
oobo share <session_id> --out session.json         # Save to file
```

### Backend Sync

```bash
oobo sync                                          # Show current sync status
oobo sync on                                       # Enable auto-sync (prompts for key if needed)
oobo sync off                                      # Disable auto-sync
oobo sync --import                                 # Import anchors from orphan branch into local DB
```

When sync is on and `OOBO_SECRET_KEY` (env var) or `api_key` is configured, anchor data syncs to the backend automatically on every commit/push.

### Auth & Remote

```bash
oobo auth login --key <api_key>                    # Authenticate with backend
oobo auth logout                                   # Remove credentials
oobo auth status                                   # Show auth state + tool keys
oobo auth set-remote https://oobo.example.com      # Self-hosted server
```

The `OOBO_SECRET_KEY` environment variable overrides the persisted `api_key` when set.

### Agent Lifecycle Hooks

```bash
# Internal plumbing — called by tool integrations, not typed by users
echo '{"session_id":"<id>","agent":"cursor","model":"claude-opus-4"}' | oobo hooks agent session-start
echo '{"session_id":"<id>"}' | oobo hooks agent stop
echo '{"session_id":"<id>"}' | oobo hooks agent session-end
```

### Maintenance

```bash
oobo scan --agent                                  # Discover projects/sessions (quiet)
oobo index                                         # Compute token counts & analytics
oobo setup                                         # Install agent hooks + git hooks
```

## Supported Tools

Cursor, Claude Code, Gemini CLI, Codex CLI, OpenCode, GitHub Copilot Chat, Windsurf, Aider, Zed AI, Trae.

## JSON Response Fields

**anchors**: `commit_hash`, `message`, `author`, `author_type`, `branch`, `committed_at`, `contributors[]` (each with `name`, `role`, `model`), `files_changed[]`, `added`, `deleted`, `file_changes[]` (each with `path`, `added`, `deleted`, `attribution` [ai/human/mixed], `agent`), `ai_added`, `ai_deleted`, `human_added`, `human_deleted`, `ai_percentage`, `sessions[]` (each with `session_id`, `agent`, `model`, `link_type`, `input_tokens`, `output_tokens`, `files_touched[]`), `transparency_mode`, `summary`, `intent`

**sessions list**: `session_id`, `name`, `source`, `mode`, `project_path`, `created_at`, `updated_at`, `model`, `input_tokens`, `output_tokens`, `duration_secs`, `is_estimated`, `files_touched`, `tool_calls`

**sessions show**: All above plus `messages` array of `{role, text, timestamp_ms}` and `message_count`

**sessions search**: All session fields plus `matched_on` (`name` or `first_message`)

**stats**: `sessions`, `input_tokens`, `output_tokens`, `total_tokens`, `per_tool[]`, `per_model[]`, `ai_code`, `productivity`, `daily[]`

**projects list**: `id`, `name`, `path`, `tools`, `sessions`, `input_tokens`, `output_tokens`

**share**: `session_id`, `source`, `model`, `messages[]` (redacted), `stats`, `shared_at`, `oobo_version`

## Agent Configuration

Agents should operate with **transparency on** — this is the recommended default for all automated workflows. Transparency on means anchor metadata and redacted transcripts sync to the orphan branch, giving teams complete visibility into AI contributions.

### First-time setup

If oobo is not yet configured, write the config file directly:

```bash
mkdir -p ~/.oobo
cat > ~/.oobo/config.toml << 'EOF'
[git]
alias_enabled = true

[transparency]
mode = "on"

[server]
sync = false
EOF
```

Then install the git alias and hooks:

```bash
oobo alias install
oobo scan
```

### Recommended agent settings

| Setting | Value | Why |
|---------|-------|-----|
| `transparency.mode` | `on` | Metadata + redacted transcripts sync |
| `git.alias_enabled` | `true` | Automatic enrichment on every commit |
| `server.sync` | `true` | Auto-sync anchor data to backend on commit/push |
| `--agent` flag | Always use | Structured JSON output for parsing |

### Verifying configuration

```bash
oobo inspect --agent                               # Check all settings, hooks, tools
oobo inspect --fix                                 # Auto-fix common issues
```

## Architecture

Oobo operates in two layers:

- **Level 1 (Git Decorator)**: Decorates git to intercept commits, enrich them with anchor metadata (linked sessions, agent/human attribution, diff stats), and sync metadata via the `oobo/anchors/v1` orphan branch.
- **Level 2 (Local Memory Engine)**: Maintains a local SQLite database with session history, token counts, analytics, and time-series data across all AI tools.

## Notes

- Token counts with `is_estimated: true` are tiktoken estimates. `false` means native from the tool.
- Session IDs support prefix matching (e.g. `2c97` matches `2c97dced-3950-...`).
- Run `oobo scan` then `oobo index` if data seems stale.
- All data is local SQLite at `~/.oobo/db/oobo.db`.
- Anchor metadata is stored per-commit and visible via `oobo anchors --agent`.
- `git log` passes through to git normally; `oobo anchors` is the enriched alternative.
