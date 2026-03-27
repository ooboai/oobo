# JSON Response Fields (--json)

**anchors**: `commit_hash`, `message`, `author`, `author_type`, `branch`, `committed_at`, `contributors[]` (each with `name`, `role`, `model`), `files_changed[]`, `added`, `deleted`, `file_changes[]` (each with `path`, `added`, `deleted`, `attribution` [ai/human/mixed], `agent`), `ai_added`, `ai_deleted`, `human_added`, `human_deleted`, `ai_percentage`, `sessions[]` (each with `session_id`, `agent`, `model`, `link_type`, `input_tokens`, `output_tokens`, `files_touched[]`, `parent_session_id?`, `subagent_type?`, `peer_session_ids[]`), `transparency_mode`, `summary`, `intent`, `file_interactions[]?` (each with `path`, `sessions[]` of `{session_id, role}` where role is writer/reader/both)

**sessions list**: `session_id`, `name`, `source`, `mode`, `project_path`, `created_at`, `updated_at`, `model`, `input_tokens`, `output_tokens`, `duration_secs`, `is_estimated`, `files_touched`, `tool_calls`, `parent_session_id?`, `subagent_type?`, `peer_session_ids[]?`

**sessions show**: All above plus `messages` array of `{role, text, timestamp_ms}`, `message_count`, and `peer_session_ids[]?`

**sessions search**: All session fields plus `matched_on` (`name` or `first_message`) and `peer_session_ids[]?`

**stats**: `sessions`, `input_tokens`, `output_tokens`, `total_tokens`, `per_tool[]`, `per_model[]`, `ai_code`, `productivity`, `daily[]`

**projects list**: `id`, `name`, `path`, `tools`, `sessions`, `input_tokens`, `output_tokens`

**share**: `session_id`, `source`, `model`, `messages[]` (redacted), `stats`, `shared_at`, `oobo_version`

**blame**: `path`, `added`, `deleted`, `attribution` (ai/human/mixed), `agent`, `line_attributions[]` (each with `author`, `ranges[]` of `{start, end}`, `agent?`)
