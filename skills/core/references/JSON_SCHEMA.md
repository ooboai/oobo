# JSON Response Fields (--json)

These fields describe the current 1.0 command surface. Removed 0.1.x commands such as `sessions`, `projects`, `stats`, and `share` are not public commands.

**anchors list**: array of objects with `sha`, `project`, `subject`, `timestamp`, `tool`, `tokens`, `sessions`, `ai_pct`.

**anchor show**: object with `sha`, `parents[]`, `timestamp`, `author.raw`, `subject`, `tools[]`, `tokens.{input,output,cache_read,cache_write,total}`, `attribution.{ai_lines,human_lines,ai_pct}`, `sessions[]`, `files_changed[]`.

**search**: object containing `query`, `sources[]`, `total_hits`, and `hits[]` for local and/or remote session/anchor matches. Use `--local`, `--remote`, or `--both` to control sources.

**bare `oobo` outside a repo**: object with `projects[]` and aggregate `stats`. Each project includes `id`, `name`, `path`, `remote`, `enabled`, `last_activity`, `anchors`, `tokens`, `ai_pct`.

**settings**: object/array of effective setting rows. Valid keys are `key`, `api_url`, `remote`, `transparency`, `tools.experimental`, `setup.scan_roots`.

**blame**: per-file/per-line attribution data including path, line numbers, commit metadata, and AI attribution when available. Machine-output git blame formats bypass the AI overlay.

**goto**: object with `action: "goto"`, `target` (label), `stashed` (bool), `memory_path` (optional path to materialized turn memory).

**back**: object with `action: "back"`, `label` (where you returned to), `stash_applied` (bool), `remaining_depth` (int - how many more entries in the stack).
