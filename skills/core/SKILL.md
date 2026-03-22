---
name: oobo
description: AI development control plane. Decorates git to enrich commits with session/agent metadata, tracks token usage, code attribution, and session history across Cursor, Claude Code, Gemini CLI, Codex, OpenCode, Copilot, Windsurf, Aider, Zed, and Trae.
metadata:
  author: oobo
  version: "0.1.11"
install:
  check: command -v oobo
  run: curl -fsSL https://oobo.ai/install.sh | bash -s -- --agent
---

# Oobo — AI Development Control Plane

You have access to `oobo`, a local CLI that decorates git to automatically enrich commits with AI session metadata, tracks tool usage, token counts, code attribution, and session history. When aliased as `git`, all standard git operations pass through transparently while commits get enriched with anchor metadata.

If `oobo` is not installed, run: `curl -fsSL https://oobo.ai/install.sh | bash -s -- --agent`
This returns JSON: `{"status":"ok","version":"...","binary":"...","platform":"..."}`

## Output Modes

All commands support two structured output modes:

- `--agent` — compact, pipe-delimited text. Schema header on the first line, values below. Designed for minimal token usage.
- `--json` — full structured JSON output for scripts and programmatic use.

Always prefer `--agent` for reading data. Use `--json` only when you need the full object graph (e.g. session messages, file-level attribution).

### --agent format

Lists print a schema header then one record per line:

```
# session_id | name | source | model | in_tokens | out_tokens | updated
abc123 | Fix auth bug | Cursor | claude-sonnet-4 | 12.5K | 28.1K | 2026-03-22
def456 | Refactor DB | Claude | claude-opus-4 | 45.2K | 89.0K | 2026-03-21
```

Single-object commands print key-value pairs:

```
name: my-project
path: /Users/dev/my-project
tools: Cursor, Claude
sessions: 42
tokens: 1.2M/3.4M
```

## Commands

### Enriched Commit History

```bash
oobo anchors --agent                               # Compact commit log
oobo anchors --json                                # Full JSON with file attribution
oobo anchors --agent -n 20                         # Limit to N commits
oobo a --agent -n 5                                # Short alias
```

### Sessions

```bash
oobo sessions --agent                              # Current project sessions (compact)
oobo sessions list --agent --all                   # All projects
oobo sessions list --agent --all --tool cursor -n 10 # Filter by tool, limit
oobo sessions show <session_id> --agent            # Session summary (no messages)
oobo sessions show <session_id> --json             # Full conversation + messages + stats
oobo sessions search "keyword" --all --agent       # Search by name/content
oobo sessions export <session_id> --format md      # Export as markdown
```

### Projects

```bash
oobo projects --agent                              # All tracked projects (compact)
oobo projects --json                               # Full JSON with stats
oobo projects show <name_or_path> --agent          # Project summary
```

### Stats & Analytics

```bash
oobo stats --agent                                 # Global stats (compact)
oobo stats --json                                  # Full JSON with breakdowns
oobo stats --project <name> --agent                # Per-project
oobo stats --tool cursor --agent                   # Per-tool
oobo stats --since 7d --agent                      # Time-scoped
```

### AI Development Infographic

```bash
oobo card --agent                                  # Stats summary (compact)
oobo card --json                                   # Full JSON card data
oobo card --out card.svg                           # Save SVG infographic to custom path
oobo card --format md --out card.md                # Save markdown card
```

### Data Sources

```bash
oobo sources --agent                               # Data source coverage (compact)
oobo sources --json                                # Full JSON
oobo dash --agent                                  # Configuration overview (compact)
oobo dash --json                                   # Full JSON
```

### Diagnostics

```bash
oobo inspect --agent                               # Diagnostics (compact)
oobo inspect --json                                # Full JSON
oobo inspect --fix                                 # Auto-fix common issues
oobo version --agent                               # Just the version string
oobo version --json                                # Version info as JSON
```

### Share Sessions

```bash
oobo share <session_id> --agent                    # Share + compact response
oobo share <session_id> --out chat.md              # Save redacted session as markdown
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

The default remote is `api.oobo.ai` (free). Self-hosted servers are also supported.

```bash
oobo auth login --key <api_key>                    # Authenticate (free account at oobo.ai)
oobo auth logout                                   # Remove credentials
oobo auth status                                   # Show auth state + tool keys
oobo auth set-remote https://oobo.example.com      # Point to self-hosted server
```

The `OOBO_SECRET_KEY` environment variable overrides the persisted `api_key` when set.

### Remote API Surface

Remotes implement endpoints under `/anchors`. Only ingest is required:

| Endpoint | Method | Auth | Required | Purpose |
|----------|--------|------|----------|---------|
| `/anchors/ingest` | POST | Bearer | **Yes** | Accept anchor data on commit |
| `/anchors/verify` | GET | Bearer | No | Validate API key |
| `/anchors/health` | GET | None | No | Connectivity check |
| `/anchors/share` | POST | Bearer optional | No | Share a redacted session |

### Agent Lifecycle Hooks

```bash
# Internal plumbing — called by tool integrations, not typed by users
echo '{"session_id":"<id>","agent":"cursor","model":"claude-opus-4"}' | oobo hooks agent session-start
echo '{"session_id":"<id>","tool_name":"Read","file_path":"/src/main.rs"}' | oobo hooks agent after-tool-use --tool cursor
echo '{"session_id":"<id>","tool_name":"Edit"}' | oobo hooks agent tool-use-failure --tool claude
echo '{"session_id":"<id>","subagent_id":"sub-1","subagent_type":"explore"}' | oobo hooks agent subagent-start --tool cursor
echo '{"session_id":"<id>","duration_ms":1500}' | oobo hooks agent after-agent-thought --tool cursor
echo '{"session_id":"<id>"}' | oobo hooks agent pre-compact --tool cursor
echo '{"session_id":"<id>"}' | oobo hooks agent stop
echo '{"session_id":"<id>"}' | oobo hooks agent session-end
```

### Maintenance

```bash
oobo scan                                          # Discover projects/sessions
oobo index                                         # Compute token counts & analytics
oobo setup                                         # Install agent hooks + git hooks
oobo update                                        # Self-update + run migrations
```

## Supported Tools

Cursor, Claude Code, Gemini CLI, Codex CLI, OpenCode, GitHub Copilot Chat, Windsurf, Aider, Zed AI, Trae.

## JSON Response Fields (--json)

**anchors**: `commit_hash`, `message`, `author`, `author_type`, `branch`, `committed_at`, `contributors[]` (each with `name`, `role`, `model`), `files_changed[]`, `added`, `deleted`, `file_changes[]` (each with `path`, `added`, `deleted`, `attribution` [ai/human/mixed], `agent`), `ai_added`, `ai_deleted`, `human_added`, `human_deleted`, `ai_percentage`, `sessions[]` (each with `session_id`, `agent`, `model`, `link_type`, `input_tokens`, `output_tokens`, `files_touched[]`, `parent_session_id?`, `subagent_type?`, `peer_session_ids[]`), `transparency_mode`, `summary`, `intent`, `file_interactions[]?` (each with `path`, `sessions[]` of `{session_id, role}` where role is writer/reader/both)

**sessions list**: `session_id`, `name`, `source`, `mode`, `project_path`, `created_at`, `updated_at`, `model`, `input_tokens`, `output_tokens`, `duration_secs`, `is_estimated`, `files_touched`, `tool_calls`, `parent_session_id?`, `subagent_type?`, `peer_session_ids[]?`

**sessions show**: All above plus `messages` array of `{role, text, timestamp_ms}`, `message_count`, and `peer_session_ids[]?`

**sessions search**: All session fields plus `matched_on` (`name` or `first_message`) and `peer_session_ids[]?`

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
| `--agent` flag | Always use | Compact output, low token cost |
| `--json` flag | When needed | Full structured data for parsing |

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
- Stats are computed proactively at session-end and commit time. Run `oobo scan` then `oobo index` only if data seems stale after an upgrade.
- All data is local SQLite at `~/.oobo/db/oobo.db`.
- Anchor metadata is stored per-commit and visible via `oobo anchors --agent`.
- `git log` passes through to git normally; `oobo anchors` is the enriched alternative.
- `oobo update` automatically runs post-update migrations (skill file refresh, DB migrations, hook reinstall).
