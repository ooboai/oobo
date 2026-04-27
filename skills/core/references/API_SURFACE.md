# Remote API Surface

Remotes implement endpoints under `/anchors`. The CLI only calls search (authenticated) and health (unauthenticated). There is no cloud upload/ingest pipeline; team sync is Git-first via the orphan branch.

| Endpoint | Method | Auth | Required | Purpose |
|----------|--------|------|----------|---------|
| `/anchors/search` | POST | Bearer | **Yes** | Search anchors/sessions |
| `/anchors/health` | GET | None | No | Connectivity check |

## Agent Lifecycle Hooks

```bash
# Internal plumbing — called by tool integrations, not typed by users
echo '{"session_id":"<id>","agent":"cursor","model":"claude-opus-4"}' | anchor hooks agent session-start --tool cursor
echo '{"session_id":"<id>","tool_name":"Read","file_path":"/src/main.rs"}' | anchor hooks agent after-tool-use --tool cursor
echo '{"session_id":"<id>","tool_name":"Edit"}' | anchor hooks agent tool-use-failure --tool claude
echo '{"session_id":"<id>","subagent_id":"sub-1","subagent_type":"explore"}' | anchor hooks agent subagent-start --tool cursor
echo '{"session_id":"<id>","duration_ms":1500}' | anchor hooks agent after-agent-thought --tool cursor
echo '{"session_id":"<id>"}' | anchor hooks agent pre-compact --tool cursor
echo '{"session_id":"<id>"}' | anchor hooks agent stop --tool cursor
echo '{"session_id":"<id>"}' | anchor hooks agent session-end --tool cursor
```
