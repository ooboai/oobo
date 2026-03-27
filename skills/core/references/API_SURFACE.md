# Remote API Surface

Remotes implement endpoints under `/anchors`. Only ingest is required:

| Endpoint | Method | Auth | Required | Purpose |
|----------|--------|------|----------|---------|
| `/anchors/ingest` | POST | Bearer | **Yes** | Accept anchor data on commit |
| `/anchors/verify` | GET | Bearer | No | Validate API key |
| `/anchors/health` | GET | None | No | Connectivity check |
| `/anchors/share` | POST | Bearer optional | No | Share a redacted session |

## Agent Lifecycle Hooks

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
