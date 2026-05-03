# Remote API Surface

Remotes implement endpoints under `/anchors`. The CLI only calls search (authenticated) and health (unauthenticated). There is no cloud upload/ingest pipeline; team sync is Git-first via the orphan branch.

| Endpoint | Method | Auth | Required | Purpose |
|----------|--------|------|----------|---------|
| `/anchors/search` | POST | Bearer | **Yes** | Search anchors/sessions |
| `/anchors/health` | GET | None | No | Connectivity check |

## Agent Lifecycle Hooks

```bash
# Internal plumbing — called by tool integrations, not typed by users
echo '{"session_id":"<id>","agent":"cursor","model":"claude-opus-4"}' | oobo hooks agent session-start --tool cursor
echo '{"session_id":"<id>","tool_name":"Read","file_path":"/src/main.rs"}' | oobo hooks agent after-tool-use --tool cursor
echo '{"session_id":"<id>","tool_name":"Edit"}' | oobo hooks agent tool-use-failure --tool claude
echo '{"session_id":"<id>","subagent_id":"sub-1","subagent_type":"explore"}' | oobo hooks agent subagent-start --tool cursor
echo '{"session_id":"<id>","duration_ms":1500}' | oobo hooks agent after-agent-thought --tool cursor
echo '{"session_id":"<id>"}' | oobo hooks agent pre-compact --tool cursor
echo '{"session_id":"<id>"}' | oobo hooks agent stop --tool cursor
echo '{"session_id":"<id>"}' | oobo hooks agent session-end --tool cursor
```
