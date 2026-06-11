//! `anchors/v2` — the durable attribution store.
//!
//! One orphan branch holds three layers:
//!
//! ```text
//! anchors/v2/
//!   repos/<repo_key>/
//!     anchors/<sha[0:2]>/<sha[2:]>/
//!       anchor.json                  # enriched commit + session refs + coverage
//!       timeline.json                # optional
//!     provenance/<sid[0:2]>/<sid[2:]>/
//!       session.json                 # stub: uid, tool, home_location (MUTABLE)
//!       turns/<NNNNNN>/              # immutable once written
//!         turn.json                  # tokens, model, trigger, timestamps
//!         edits.json                 # per-file pre/post blobs (THIS repo's files)
//!     index/
//!       anchors-by-time.jsonl        # ts \t sha          — ordering only
//!       sessions-by-time.jsonl       # ts \t session_uid  — ordering only
//!   sessions/<sid[0:2]>/<sid[2:]>/   # CONVERSATION LAYER — home store only
//!     session.json                   # full metadata (MUTABLE)
//!     turns/<NNNNNN>/
//!       transcript.json              # sanitized native transcript slice
//!       tool_calls.json              # sanitized tool inputs/outputs
//!   index/
//!     sessions.jsonl                 # session_uid → home + repos touched
//! ```
//!
//! Design rules:
//! - **Id-address for lookup, timestamp only for ordering.** Every canonical
//!   path is derivable from a key you already hold (sha, session_uid) — O(1)
//!   `git show` lookups, no tree walks.
//! - **Prefix fan-out** keeps git tree nodes small (like `.git/objects`).
//! - **Mutability boundary:** `turns/NNNNNN/` is immutable; `session.json`
//!   is the only mutable file and merges per-field (sets union, counters
//!   max, scalars LWW) so concurrent writers converge.
//! - **Conversation stored exactly once** — in the session's home store.
//!   Edited repos hold provenance + pointers only.
//! - **Everything passes [`Publisher::publish`]** before reaching the branch.

pub mod resolve;

use serde::{Deserialize, Serialize};

use crate::core::identity::SessionLineage;
use crate::core::turn::{TurnFileSnapshot, TurnTokens};
use crate::error::CliError;

use super::{
    branch_exists_named, ensure_branch_named, git_in, read_from_branch_named, shard_key,
    write_to_branch_named,
};

pub const BRANCH: &str = "oobo/anchors/v2";
pub const V2_SCHEMA_VERSION: u32 = 2;

// ── Records ────────────────────────────────────────────────────────────

/// Pointer from an anchor to a session, with enough information to find
/// the conversation layer (which may live in a different store).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionRef {
    pub session_uid: String,
    /// Where the conversation layer lives: the session's home store
    /// (anchor remote URL or repo id). `None` = this store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_location: Option<String>,
    /// Turns from this session claimed by this anchor.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turn_uids: Vec<String>,
}

/// Which capture machinery was active when an anchor was built — the
/// honesty manifest. Readers use it to qualify confidence ("hooks were
/// firing for cursor + claude; 2 files had capture gaps").
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CoverageManifest {
    /// Tools with live hook capture during the contributing sessions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// Hook event names observed during the contributing turns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hook_events_seen: Vec<String>,
    /// Files whose edit chains had capture gaps at turn end.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capture_gap_files: Vec<String>,
    #[serde(default)]
    pub recorded_at: i64,
}

/// The v2 anchor: the existing enriched-commit shape plus session refs
/// (pointers, never embedded copies) and the coverage manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorRecord {
    #[serde(flatten)]
    pub anchor: crate::core::anchor::Anchor,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub session_refs: Vec<SessionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<CoverageManifest>,
}

/// Session metadata. Used in both layers: the provenance layer stores a
/// stub (identity + pointers); the home store's conversation layer stores
/// the full record. The ONLY mutable file in the layout — merged
/// per-field, never blind-overwritten.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    #[serde(default)]
    pub schema_version: u32,
    pub session_uid: String,
    /// Native ids that map to this session (resume/compaction can create
    /// several). Set-union on merge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_session_ids: Vec<String>,
    #[serde(default)]
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Where the conversation layer lives. `None` = this store is home.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_location: Option<String>,
    /// Project the session originated in (its home repo).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_repo_id: Option<String>,
    /// Every repo this session edited. Set-union on merge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos_touched: Vec<String>,
    #[serde(default, skip_serializing_if = "SessionLineage::is_empty")]
    pub lineage: SessionLineage,
    /// Highest turn count observed. Counter-max on merge.
    #[serde(default)]
    pub turn_count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub started_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
}

/// Provenance-layer turn metadata (`turn.json`). Immutable once written.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TurnRecord {
    #[serde(default)]
    pub schema_version: u32,
    pub turn_uid: String,
    pub session_uid: String,
    /// Index within this store's turn sequence for the session (the
    /// directory name). Repo-local: each repo only counts turns that
    /// touched it.
    pub turn_index: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_turn_index: Option<i64>,
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Sanitized preview of the prompt/instruction that triggered this
    /// turn (already passed through the publish choke-point).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    #[serde(default)]
    pub tokens: TurnTokens,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_names: Vec<String>,
    #[serde(default)]
    pub capture_gap: bool,
}

/// Provenance-layer per-file edit evidence (`edits.json`). Immutable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TurnEdits {
    #[serde(default)]
    pub files: Vec<TurnFileSnapshot>,
}

// ── Path derivation (the O(1) contract) ────────────────────────────────

/// Tree-safe key for a project id. Project ids contain `/` and `:`
/// (`r:github.com/acme/widget`), so we slug them and append a short
/// content hash to rule out slug collisions while staying readable.
pub fn repo_key(project_id: &str) -> String {
    let slug: String = project_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let h = crate::core::turn::fnv1a64(project_id.as_bytes());
    format!(
        "{}-{:08x}",
        slug.trim_matches('-'),
        (h >> 32) as u32 ^ h as u32
    )
}

fn anchor_dir(repo_key: &str, sha: &str) -> String {
    let (p, rest) = shard_key(sha);
    format!("repos/{repo_key}/anchors/{p}/{rest}")
}

fn provenance_session_dir(repo_key: &str, session_uid: &str) -> String {
    let (p, rest) = shard_key(session_uid);
    format!("repos/{repo_key}/provenance/{p}/{rest}")
}

fn conversation_session_dir(session_uid: &str) -> String {
    let (p, rest) = shard_key(session_uid);
    format!("sessions/{p}/{rest}")
}

fn turn_dirname(index: i64) -> String {
    format!("{index:06}")
}

fn repo_index_path(repo_key: &str, name: &str) -> String {
    format!("repos/{repo_key}/index/{name}")
}

const GLOBAL_SESSIONS_INDEX: &str = "index/sessions.jsonl";

// ── Publish choke-point ─────────────────────────────────────────────────

/// The single gate every byte must pass before reaching the orphan branch
/// (or the backend). Secret redaction (gitleaks when present, bundled
/// baseline ruleset always) + absolute-path stripping.
pub struct Publisher {
    project_root: String,
    /// When true, refuse to publish content in which a secret was found
    /// (rather than redact-and-hope).
    pub block_on_secret: bool,
}

impl Publisher {
    pub fn new(project_root: &str) -> Self {
        Self {
            project_root: project_root.to_string(),
            block_on_secret: false,
        }
    }

    /// Sanitize free text destined for shared storage. Errors only in
    /// block-on-secret mode when a secret was detected.
    pub fn publish(&self, text: &str) -> Result<String, CliError> {
        let clean = crate::redact::sanitize_for_public(text, &self.project_root);
        if self.block_on_secret && clean.contains("[REDACTED]") && !text.contains("[REDACTED]") {
            return Err(CliError::Git(
                "refusing to publish: secret detected in content destined for shared storage"
                    .into(),
            ));
        }
        Ok(clean)
    }
}

// ── Writes ──────────────────────────────────────────────────────────────

/// Write (or overwrite) an anchor record. Also appends to the repo's
/// `anchors-by-time` index.
pub fn write_anchor(
    project_root: &str,
    repo_id: &str,
    record: &AnchorRecord,
    timeline_json: Option<&str>,
) -> Result<(), CliError> {
    ensure_branch_named(project_root, BRANCH)?;
    let publisher = Publisher::new(project_root);
    let key = repo_key(repo_id);
    let dir = anchor_dir(&key, &record.anchor.commit_hash);

    let json = serde_json::to_string_pretty(record)
        .map_err(|e| CliError::Git(format!("serialize anchor: {e}")))?;
    let mut entries = vec![(format!("{dir}/anchor.json"), publisher.publish(&json)?)];
    if let Some(timeline) = timeline_json {
        entries.push((format!("{dir}/timeline.json"), publisher.publish(timeline)?));
    }

    let index = repo_index_path(&key, "anchors-by-time.jsonl");
    let line = format!(
        "{}\t{}",
        record.anchor.committed_at, record.anchor.commit_hash
    );
    entries.push(appended_index(project_root, &index, &line));

    write_to_branch_named(project_root, BRANCH, &entries)
}

/// Write a session record into the **provenance layer** of `repo_id`.
/// Merges per-field with any existing record (concurrent writers converge).
pub fn write_provenance_session(
    project_root: &str,
    repo_id: &str,
    session: &SessionRecord,
) -> Result<(), CliError> {
    ensure_branch_named(project_root, BRANCH)?;
    let key = repo_key(repo_id);
    let dir = provenance_session_dir(&key, &session.session_uid);
    let path = format!("{dir}/session.json");

    let merged = match read_json::<SessionRecord>(project_root, &path) {
        Some(existing) => merge_sessions(&existing, session),
        None => session.clone(),
    };
    let json = serialize_session(project_root, &merged)?;

    let mut entries = vec![(path, json)];
    let index = repo_index_path(&key, "sessions-by-time.jsonl");
    let line = format!("{}\t{}", merged.updated_at, merged.session_uid);
    entries.push(appended_index(project_root, &index, &line));

    write_to_branch_named(project_root, BRANCH, &entries)
}

/// Write an immutable provenance turn (turn.json + edits.json) under
/// `repo_id`. Returns `Ok(false)` without writing when the turn already
/// exists — immutability is enforced, re-runs are no-ops.
pub fn write_provenance_turn(
    project_root: &str,
    repo_id: &str,
    turn: &TurnRecord,
    edits: &TurnEdits,
) -> Result<bool, CliError> {
    ensure_branch_named(project_root, BRANCH)?;
    let publisher = Publisher::new(project_root);
    let key = repo_key(repo_id);
    let dir = format!(
        "{}/turns/{}",
        provenance_session_dir(&key, &turn.session_uid),
        turn_dirname(turn.turn_index)
    );
    let turn_path = format!("{dir}/turn.json");
    if read_from_branch_named(project_root, BRANCH, &turn_path).is_some() {
        return Ok(false);
    }

    let turn_json = serde_json::to_string_pretty(turn)
        .map_err(|e| CliError::Git(format!("serialize turn: {e}")))?;
    let edits_json = serde_json::to_string_pretty(edits)
        .map_err(|e| CliError::Git(format!("serialize edits: {e}")))?;

    write_to_branch_named(
        project_root,
        BRANCH,
        &[
            (turn_path, publisher.publish(&turn_json)?),
            (format!("{dir}/edits.json"), publisher.publish(&edits_json)?),
        ],
    )?;
    Ok(true)
}

/// Write a session record into the **conversation layer** (home store
/// only). Merges per-field with any existing record, and updates the
/// global sessions index.
pub fn write_conversation_session(
    project_root: &str,
    session: &SessionRecord,
) -> Result<(), CliError> {
    ensure_branch_named(project_root, BRANCH)?;
    let dir = conversation_session_dir(&session.session_uid);
    let path = format!("{dir}/session.json");

    let merged = match read_json::<SessionRecord>(project_root, &path) {
        Some(existing) => merge_sessions(&existing, session),
        None => session.clone(),
    };
    let json = serialize_session(project_root, &merged)?;

    let index_line = serde_json::json!({
        "session_uid": merged.session_uid,
        "home": merged.home_location,
        "repos": merged.repos_touched,
    })
    .to_string();

    write_to_branch_named(
        project_root,
        BRANCH,
        &[
            (path, json),
            appended_index(project_root, GLOBAL_SESSIONS_INDEX, &index_line),
        ],
    )
}

/// Write an immutable conversation turn (transcript + tool calls) into
/// the home store. Both payloads pass the publish choke-point. Returns
/// `Ok(false)` when the turn already exists.
pub fn write_conversation_turn(
    project_root: &str,
    session_uid: &str,
    turn_index: i64,
    transcript_json: &str,
    tool_calls_json: &str,
) -> Result<bool, CliError> {
    ensure_branch_named(project_root, BRANCH)?;
    let publisher = Publisher::new(project_root);
    let dir = format!(
        "{}/turns/{}",
        conversation_session_dir(session_uid),
        turn_dirname(turn_index)
    );
    let transcript_path = format!("{dir}/transcript.json");
    if read_from_branch_named(project_root, BRANCH, &transcript_path).is_some() {
        return Ok(false);
    }

    write_to_branch_named(
        project_root,
        BRANCH,
        &[
            (transcript_path, publisher.publish(transcript_json)?),
            (
                format!("{dir}/tool_calls.json"),
                publisher.publish(tool_calls_json)?,
            ),
        ],
    )?;
    Ok(true)
}

/// Session records pass through the publisher too (titles, native ids and
/// lineage fields are free text from external tools).
fn serialize_session(project_root: &str, session: &SessionRecord) -> Result<String, CliError> {
    let publisher = Publisher::new(project_root);
    let json = serde_json::to_string_pretty(session)
        .map_err(|e| CliError::Git(format!("serialize session: {e}")))?;
    publisher.publish(&json)
}

/// Read current index content (if any) and return the entry with `line`
/// appended. Append-only by construction: existing lines are preserved
/// verbatim, duplicates tolerated (compaction dedups).
fn appended_index(project_root: &str, index_path: &str, line: &str) -> (String, String) {
    let mut content = read_from_branch_named(project_root, BRANCH, index_path)
        .map(|c| {
            let trimmed = c.trim_end().to_string();
            if trimmed.is_empty() {
                String::new()
            } else {
                format!("{trimmed}\n")
            }
        })
        .unwrap_or_default();
    content.push_str(line);
    content.push('\n');
    (index_path.to_string(), content)
}

// ── Reads (O(1) by id) ──────────────────────────────────────────────────

fn read_json<T: serde::de::DeserializeOwned>(project_root: &str, path: &str) -> Option<T> {
    let content = read_from_branch_named(project_root, BRANCH, path)?;
    serde_json::from_str(&content).ok()
}

pub fn read_anchor(project_root: &str, repo_id: &str, sha: &str) -> Option<AnchorRecord> {
    let dir = anchor_dir(&repo_key(repo_id), sha);
    read_json(project_root, &format!("{dir}/anchor.json"))
}

/// Re-key v2 anchor records after a history rewrite, using git's exact
/// old→new pairs from the `post-rewrite` hook. Only the sha-keyed anchor
/// layer needs this — provenance and conversation layers are keyed by
/// session/turn identity and survive rewrites untouched (claims are
/// content-based by design).
///
/// The old record is left in place: rewritten-away shas are simply
/// unreachable, and a fast-forward back to them would find their anchor
/// intact.
pub fn rekey_anchors_from_pairs(
    project_root: &str,
    repo_id: &str,
    pairs: &[(String, String)],
) -> Result<(), CliError> {
    if pairs.is_empty() || !branch_exists(project_root) {
        return Ok(());
    }
    let key = repo_key(repo_id);
    for (old, new) in pairs {
        if old == new {
            continue;
        }
        let Some(mut record) = read_anchor(project_root, repo_id, old) else {
            continue;
        };
        if read_anchor(project_root, repo_id, new).is_some() {
            continue; // replay — already rekeyed
        }
        record.anchor.commit_hash.clone_from(new);
        let old_dir = anchor_dir(&key, old);
        let timeline =
            read_from_branch_named(project_root, BRANCH, &format!("{old_dir}/timeline.json"));
        write_anchor(project_root, repo_id, &record, timeline.as_deref())?;
    }
    Ok(())
}

pub fn read_provenance_session(
    project_root: &str,
    repo_id: &str,
    session_uid: &str,
) -> Option<SessionRecord> {
    let dir = provenance_session_dir(&repo_key(repo_id), session_uid);
    read_json(project_root, &format!("{dir}/session.json"))
}

pub fn read_provenance_turn(
    project_root: &str,
    repo_id: &str,
    session_uid: &str,
    turn_index: i64,
) -> Option<(TurnRecord, TurnEdits)> {
    let dir = format!(
        "{}/turns/{}",
        provenance_session_dir(&repo_key(repo_id), session_uid),
        turn_dirname(turn_index)
    );
    let turn = read_json::<TurnRecord>(project_root, &format!("{dir}/turn.json"))?;
    let edits =
        read_json::<TurnEdits>(project_root, &format!("{dir}/edits.json")).unwrap_or_default();
    Some((turn, edits))
}

/// `ls-tree` a directory on the store branch, falling back to the
/// remote-tracking ref (fresh clones carry the store under origin only).
fn ls_store_dir(project_root: &str, dir: &str) -> Option<String> {
    git_in(
        project_root,
        &["ls-tree", "--name-only", BRANCH, &format!("{dir}/")],
    )
    .or_else(|_| {
        git_in(
            project_root,
            &[
                "ls-tree",
                "--name-only",
                &format!("refs/remotes/origin/{BRANCH}"),
                &format!("{dir}/"),
            ],
        )
    })
    .ok()
}

/// Enumerate every stored provenance turn for a session (ascending
/// index). One `ls-tree` for the directory, then O(1) reads per turn.
pub fn list_provenance_turns(
    project_root: &str,
    repo_id: &str,
    session_uid: &str,
) -> Vec<(TurnRecord, TurnEdits)> {
    let dir = format!(
        "{}/turns",
        provenance_session_dir(&repo_key(repo_id), session_uid)
    );
    let Some(listing) = ls_store_dir(project_root, &dir) else {
        return Vec::new();
    };
    let mut indices: Vec<i64> = listing
        .lines()
        .filter_map(|l| l.rsplit('/').next())
        .filter_map(|name| name.parse().ok())
        .collect();
    indices.sort_unstable();
    indices
        .into_iter()
        .filter_map(|idx| read_provenance_turn(project_root, repo_id, session_uid, idx))
        .collect()
}

/// Indices of conversation turns stored for a session (ascending).
pub fn list_conversation_turn_indices(project_root: &str, session_uid: &str) -> Vec<i64> {
    let dir = format!("{}/turns", conversation_session_dir(session_uid));
    let Some(listing) = ls_store_dir(project_root, &dir) else {
        return Vec::new();
    };
    let mut indices: Vec<i64> = listing
        .lines()
        .filter_map(|l| l.rsplit('/').next())
        .filter_map(|name| name.parse().ok())
        .collect();
    indices.sort_unstable();
    indices
}

pub fn read_conversation_session(project_root: &str, session_uid: &str) -> Option<SessionRecord> {
    let dir = conversation_session_dir(session_uid);
    read_json(project_root, &format!("{dir}/session.json"))
}

pub fn read_conversation_turn(
    project_root: &str,
    session_uid: &str,
    turn_index: i64,
) -> Option<(String, String)> {
    let dir = format!(
        "{}/turns/{}",
        conversation_session_dir(session_uid),
        turn_dirname(turn_index)
    );
    let transcript =
        read_from_branch_named(project_root, BRANCH, &format!("{dir}/transcript.json"))?;
    let tool_calls =
        read_from_branch_named(project_root, BRANCH, &format!("{dir}/tool_calls.json"))
            .unwrap_or_default();
    Some((transcript, tool_calls))
}

/// Recency listing for a repo's anchors — straight from the append-only
/// index, newest first, deduped. No tree walk.
pub fn list_anchors_by_time(project_root: &str, repo_id: &str) -> Vec<(i64, String)> {
    read_time_index(
        project_root,
        &repo_index_path(&repo_key(repo_id), "anchors-by-time.jsonl"),
    )
}

/// Recency listing for a repo's sessions (provenance layer).
pub fn list_sessions_by_time(project_root: &str, repo_id: &str) -> Vec<(i64, String)> {
    read_time_index(
        project_root,
        &repo_index_path(&repo_key(repo_id), "sessions-by-time.jsonl"),
    )
}

fn read_time_index(project_root: &str, path: &str) -> Vec<(i64, String)> {
    let Some(content) = read_from_branch_named(project_root, BRANCH, path) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<(i64, String)> = content
        .lines()
        .filter_map(|line| {
            let (ts, id) = line.split_once('\t')?;
            Some((ts.parse::<i64>().ok()?, id.to_string()))
        })
        .collect();
    // Newest first; dedup keeps the newest occurrence per id.
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.retain(|(_, id)| seen.insert(id.clone()));
    out
}

/// Global session directory: `session_uid → (home, repos touched)`.
/// Last line wins per uid (the index is append-only).
pub fn read_sessions_index(
    project_root: &str,
) -> std::collections::HashMap<String, serde_json::Value> {
    let Some(content) = read_from_branch_named(project_root, BRANCH, GLOBAL_SESSIONS_INDEX) else {
        return std::collections::HashMap::new();
    };
    let mut out = std::collections::HashMap::new();
    for line in content.lines() {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(uid) = val.get("session_uid").and_then(|v| v.as_str()) {
                out.insert(uid.to_string(), val);
            }
        }
    }
    out
}

// ── Per-field merge (concurrent writers converge) ───────────────────────

/// Merge two versions of a session record. Commutative-by-construction
/// for sets and counters; scalars resolve last-writer-wins by
/// `updated_at` (ties prefer `b`, the incoming write).
pub fn merge_sessions(a: &SessionRecord, b: &SessionRecord) -> SessionRecord {
    let (older, newer) = if a.updated_at > b.updated_at {
        (b, a)
    } else {
        (a, b)
    };

    let union = |x: &[String], y: &[String]| -> Vec<String> {
        let mut set: std::collections::BTreeSet<String> =
            x.iter().chain(y.iter()).cloned().collect();
        set.retain(|s| !s.is_empty());
        set.into_iter().collect()
    };

    SessionRecord {
        schema_version: a.schema_version.max(b.schema_version),
        session_uid: newer.session_uid.clone(),
        native_session_ids: union(&a.native_session_ids, &b.native_session_ids),
        tool: pick_scalar(&newer.tool, &older.tool).clone(),
        model: pick_opt(newer.model.as_ref(), older.model.as_ref()),
        home_location: pick_opt(newer.home_location.as_ref(), older.home_location.as_ref()),
        origin_repo_id: pick_opt(newer.origin_repo_id.as_ref(), older.origin_repo_id.as_ref()),
        repos_touched: union(&a.repos_touched, &b.repos_touched),
        lineage: if newer.lineage.is_empty() {
            older.lineage.clone()
        } else {
            newer.lineage.clone()
        },
        turn_count: a.turn_count.max(b.turn_count),
        title: pick_opt(newer.title.as_ref(), older.title.as_ref()),
        started_at: match (a.started_at, b.started_at) {
            (0, x) | (x, 0) => x,
            (x, y) => x.min(y),
        },
        updated_at: a.updated_at.max(b.updated_at),
        ended_at: match (a.ended_at, b.ended_at) {
            (Some(x), Some(y)) => Some(x.max(y)),
            (x, y) => x.or(y),
        },
    }
}

fn pick_scalar<'a>(newer: &'a String, older: &'a String) -> &'a String {
    if newer.is_empty() {
        older
    } else {
        newer
    }
}

fn pick_opt(newer: Option<&String>, older: Option<&String>) -> Option<String> {
    newer.or(older).cloned()
}

/// Merge two serialized session.json payloads (used by the sync replay
/// when both sides modified the same session). Returns `None` when either
/// side fails to parse — in that case the replay keeps the remote version.
pub(in crate::git::orphan) fn merge_session_json(local: &str, remote: &str) -> Option<String> {
    let l: SessionRecord = serde_json::from_str(local).ok()?;
    let r: SessionRecord = serde_json::from_str(remote).ok()?;
    serde_json::to_string_pretty(&merge_sessions(&l, &r)).ok()
}

// ── Session home & continuation chains ─────────────────────────────────

/// Where a session's conversation layer lives, as recorded on stubs,
/// anchors and pointers: the **origin** project's configured anchor
/// remote when set, otherwise the origin repo's stable project id.
///
/// This is the two-layer write-path contract: provenance routes per
/// edited repo; the conversation syncs ONCE, to this location.
pub fn home_location_for(origin_project_root: &str) -> String {
    let global = crate::config::Config::load_or_default();
    let remote = crate::commands::sync::resolve(&global, Some(origin_project_root)).anchor_remote;

    // Remote *names* (e.g. "origin", "oobo") are machine-local; resolve
    // them to the URL they point at so the location travels.
    let url = if remote.contains("://") || remote.contains('@') || remote.contains('/') {
        Some(remote)
    } else {
        git_in(origin_project_root, &["remote", "get-url", &remote]).ok()
    };

    match url {
        Some(u) if !u.is_empty() => format!("r:{}", crate::project::canonicalize_remote(&u)),
        _ => crate::project::id_for_root(origin_project_root),
    }
}

/// Resolve the chain root for a continuation session ("one long
/// session"): follow `resumed_from`/`compacted_from` links through the
/// conversation layer until a session with no continuation lineage is
/// found. Continuation turns append under the root's object; native
/// seams stay visible as turn metadata.
///
/// Subagents (`parent_session_uid`) are NOT chained — they are distinct
/// actors and keep their own session objects.
///
/// Cycle-guarded. When a link points outside this store, the link target
/// is trusted as the root (resolution continues at read time via
/// pointers).
pub fn chain_root_uid(project_root: &str, session: &SessionRecord) -> String {
    let mut root = session.session_uid.clone();
    let mut lineage = session.lineage.clone();
    let mut seen: std::collections::HashSet<String> =
        std::collections::HashSet::from([root.clone()]);

    while let Some(parent) = lineage
        .resumed_from
        .clone()
        .or_else(|| lineage.compacted_from.clone())
    {
        if !seen.insert(parent.clone()) {
            break;
        }
        if let Some(rec) = read_conversation_session(project_root, &parent) {
            root.clone_from(&rec.session_uid);
            lineage = rec.lineage;
        } else {
            root = parent;
            break;
        }
    }
    root
}

// ── Maintenance ─────────────────────────────────────────────────────────

/// Squash the orphan branch to a single rootless commit holding the
/// current tree. History on this branch is worthless — the tip is
/// everything — and squashing keeps clones and pushes cheap forever.
pub fn squash_to_tip(project_root: &str) -> Result<(), CliError> {
    if !branch_exists_named(project_root, BRANCH) {
        return Ok(());
    }
    let tip = git_in(project_root, &["rev-parse", BRANCH])?;
    // Already a root commit → nothing to do.
    if git_in(project_root, &["rev-parse", &format!("{tip}^")]).is_err() {
        return Ok(());
    }
    let tree = git_in(project_root, &["rev-parse", &format!("{tip}^{{tree}}")])?;
    let squashed = super::git_stdin_in(
        project_root,
        &["commit-tree", &tree],
        "oobo: anchors/v2 tip",
    )?;
    git_in(
        project_root,
        &[
            "update-ref",
            &format!("refs/heads/{BRANCH}"),
            &squashed,
            &tip,
        ],
    )?;
    Ok(())
}

/// Number of commits on the v2 branch (test/diagnostic helper).
pub fn branch_depth(project_root: &str) -> usize {
    git_in(project_root, &["rev-list", "--count", BRANCH])
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

// ── Sync (delegates to the shared union-merge machinery) ───────────────

pub fn push(project_root: &str) -> Result<(), CliError> {
    super::sync::push_branch(project_root, BRANCH)
}

pub fn fetch_and_reconcile(project_root: &str) -> Result<(), CliError> {
    super::sync::fetch_and_reconcile_branch(project_root, BRANCH)
}

pub fn remote_branch_exists(project_root: &str) -> bool {
    super::sync::remote_branch_exists_named(project_root, BRANCH)
}

pub fn branch_exists(project_root: &str) -> bool {
    branch_exists_named(project_root, BRANCH)
}

#[cfg(test)]
mod tests;
