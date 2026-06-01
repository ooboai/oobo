# Remote API Surface

Remotes implement endpoints under `/anchors`. The CLI calls recall, delta, get_context, and ask (all authenticated). There is no cloud upload/ingest pipeline; team sync is Git-first via the orphan branch.

| Endpoint | Method | Auth | Required | Purpose |
|----------|--------|------|----------|---------|
| `/anchors/search` | POST | Bearer | **Yes** | Search anchors/sessions (recall) |
| `/anchors/delta` | POST | Bearer | **Yes** | Compare two anchors |
| `/anchors/context` | POST | Bearer | **Yes** | File-scoped engineering context (get_context) |
| `/anchors/ask` | POST | Bearer | **Yes** | Natural language Q&A over engineering memory |
| `/anchors/health` | GET | None | No | Connectivity check |

## MCP Server

The `oobo mcp` command starts a local stdio JSON-RPC server (MCP protocol version `2024-11-05`). It wraps the above endpoints as MCP tools and adds local-only tools (`search`, `find_related`).

Authentication: reads API key from `.oobo/secrets` (project) > `~/.oobo/config` (global) > `OOBO_SECRET_KEY` / `OOBO_API_KEY` env vars.

## Agent Lifecycle Hooks

```bash
# Internal plumbing - called by tool integrations, not typed by users
echo '{"session_id":"<id>","agent":"cursor","model":"claude-opus-4"}' | oobo hooks agent session-start --tool cursor
echo '{"session_id":"<id>","prompt":"fix the auth bug"}' | oobo hooks agent before-submit-prompt --tool cursor
echo '{"session_id":"<id>","tool_name":"Read","file_path":"/src/main.rs"}' | oobo hooks agent pre-tool-use --tool cursor
echo '{"session_id":"<id>","tool_name":"Read","file_path":"/src/main.rs"}' | oobo hooks agent after-tool-use --tool cursor
echo '{"session_id":"<id>","tool_name":"Edit","file_path":"/src/main.rs"}' | oobo hooks agent after-file-edit --tool cursor
echo '{"session_id":"<id>","tool_name":"Edit","error":"permission denied"}' | oobo hooks agent tool-use-failure --tool claude
echo '{"session_id":"<id>","duration_ms":1500}' | oobo hooks agent after-agent-thought --tool cursor
echo '{"session_id":"<id>","response":"done"}' | oobo hooks agent after-agent-response --tool cursor
echo '{"session_id":"<id>","subagent_id":"sub-1","subagent_type":"explore"}' | oobo hooks agent subagent-start --tool cursor
echo '{"session_id":"<id>","subagent_id":"sub-1"}' | oobo hooks agent subagent-stop --tool cursor
echo '{"session_id":"<id>"}' | oobo hooks agent stop --tool cursor
echo '{"session_id":"<id>"}' | oobo hooks agent session-end --tool cursor
```

All 11 events: `session-start`, `before-submit-prompt`, `pre-tool-use`, `after-tool-use`, `after-file-edit`, `tool-use-failure`, `after-agent-thought`, `after-agent-response`, `subagent-start`, `subagent-stop`, `stop`, `session-end`.
