//! Detached enrichment worker.
//!
//! The git write path appends commits to the spool ([`crate::git::spool`])
//! and spawns this worker. The worker drains the spool: for each pending
//! commit it runs the full enrichment pipeline (anchor build, content
//! claims, provenance sessions/turns, the v2 anchor record) and a
//! best-effort orphan push.
//!
//! Invariants:
//! - **Singleton per repo** via [`WorkerLock`] (stale locks stolen).
//! - **Idempotent**: a spool entry survives until durably processed;
//!   replays are no-ops (v2 turns are immutable, anchor writes converge,
//!   the attribution cursor only moves forward).
//! - **Crash-safe**: killed mid-claim, the work file remains; the next
//!   drain reprocesses it to an identical end state.

use std::collections::HashMap;
use std::path::Path;

use crate::attribution::claim;
use crate::config::Config;
use crate::core::identity;
use crate::error::CliError;
use crate::git::orphan::v2;
use crate::git::spool::{self, SpoolEntry, WorkerLock};

/// Kick the worker after spooling: detached spawn in production;
/// synchronous drain when `OOBO_WORKER_SYNC=1` (tests and debugging need
/// deterministic completion).
pub fn kick(cfg: &Config, project_root: &str) {
    if std::env::var("OOBO_WORKER_SYNC").as_deref() == Ok("1") {
        if let Err(e) = drain(cfg, project_root) {
            tracing::warn!(%e, "sync drain failed");
        }
        return;
    }
    spawn_drain(project_root);
}

/// Spawn a detached `oobo worker drain` for this repo, if there is
/// pending work and no live worker. Never blocks, never fails loudly.
pub fn spawn_drain(project_root: &str) {
    if !spool::has_pending(project_root) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = std::process::Command::new(exe)
        .args(["worker", "drain", "--root", project_root])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Drain the spool for a repo. Returns the number of commits enriched.
pub fn drain(cfg: &Config, project_root: &str) -> Result<u32, CliError> {
    drain_with_deadline(cfg, project_root, None)
}

/// Drain with an optional time budget. Returns early once the deadline is
/// reached, leaving remaining entries for the next trigger.
pub fn drain_with_deadline(
    cfg: &Config,
    project_root: &str,
    deadline: Option<std::time::Instant>,
) -> Result<u32, CliError> {
    let Some(lock) = WorkerLock::acquire(project_root) else {
        // Another worker is live; it will see our entries.
        return Ok(0);
    };

    let mut processed = 0u32;
    let mut attempted: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();

    loop {
        let work = spool::take_work(project_root);
        let mut saw_new = false;

        for file in &work {
            let entries = spool::read_entries(file);
            let mut failed: Vec<SpoolEntry> = Vec::new();
            for entry in entries {
                if let Some(dl) = deadline {
                    if std::time::Instant::now() >= dl {
                        failed.push(entry);
                        continue;
                    }
                }
                let key = (entry.root.clone(), entry.sha.clone());
                if !attempted.insert(key) {
                    // Already failed this run; keep for the next drain.
                    failed.push(entry);
                    continue;
                }
                saw_new = true;
                lock.heartbeat();
                match process_entry(cfg, &entry) {
                    EntryOutcome::Processed => processed += 1,
                    EntryOutcome::Dropped => {
                        tracing::debug!(sha = %entry.sha, "spool entry dropped (commit gone or repo disabled)");
                    }
                    EntryOutcome::Retry => {
                        tracing::warn!(sha = %entry.sha, "v2 write failed, keeping for retry");
                        failed.push(entry);
                    }
                }
            }
            spool::complete_work(file, &failed);
        }

        // Re-loop only when new entries appeared during processing.
        if !saw_new {
            break;
        }
    }

    if processed > 0 {
        crate::hooks::state::cleanup_stale(project_root, 86400);
    }

    Ok(processed)
}

enum EntryOutcome {
    Processed,
    /// Commit or repo is permanently gone — safe to discard.
    Dropped,
    /// Transient write failure — keep in spool for retry.
    Retry,
}

fn process_entry(cfg: &Config, entry: &SpoolEntry) -> EntryOutcome {
    let root = canon(&entry.root);
    let root = root.as_str();
    if !Path::new(root).exists() || cfg.is_ignored(root) || !crate::project_config::is_enabled(root)
    {
        return EntryOutcome::Dropped;
    }

    crate::project::registry_note(root);

    let probe = format!("{}^{{commit}}", entry.sha);
    if crate::git::proxy::run_git_capture_in(
        cfg,
        &["rev-parse", "--verify", "--quiet", &probe],
        Some(root),
    )
    .is_err()
    {
        return EntryOutcome::Dropped;
    }

    let Some(outcome) =
        crate::git::interceptor::enrich_commit_for(cfg, root, &entry.branch, &entry.sha)
    else {
        return EntryOutcome::Dropped;
    };

    if !write_v2(cfg, root, entry, &outcome) {
        return EntryOutcome::Retry;
    }
    EntryOutcome::Processed
}

/// Layer the v2 store writes on top of the enrichment outcome:
/// content claims, provenance sessions + turns, and the anchor record.
fn write_v2(
    cfg: &Config,
    root: &str,
    entry: &SpoolEntry,
    outcome: &crate::git::interceptor::EnrichOutcome,
) -> bool {
    let repo_id = crate::project::id_for_root(root);
    let canon_root = canon(root);

    // Claim by content against ALL edit evidence for this repo: live
    // session chains (incl. foreign-origin sessions routed here) and
    // turn-snapshot edits from finished turns.
    let sessions = crate::hooks::store::list_for_project(root);
    let pending = claim::pending_for_repo(root);
    let claim_result = claim::claim_commit(root, &entry.sha, &pending);

    // Relevant sessions = content-claimed ∪ evidence-linked.
    let mut relevant: HashMap<String, String> = HashMap::new(); // session_id → agent
    for c in &claim_result.claims {
        relevant.insert(c.session_id.clone(), c.agent.clone());
    }
    for link in &outcome.session_links {
        relevant
            .entry(link.session_id.clone())
            .or_insert_with(|| link.agent.clone());
    }
    if relevant.is_empty() && claim_result.unclaimed_paths.is_empty() {
        // Nothing changed content-wise (e.g. empty merge); still write
        // the anchor record below for completeness.
    }

    // Pull native tap turns (token deltas, model, prompts) into the
    // per-repo cache for every relevant session BEFORE the join below.
    // The hook state records exactly which artifact to read; without
    // this, tools whose taps can't self-locate (Claude) never populate
    // the cache in the live flow and v2 turns lose their tokens.
    for (sid, agent) in &relevant {
        let transcript_path = sessions
            .iter()
            .find(|s| &s.session_id == sid)
            .and_then(|s| s.transcript_path.clone());
        crate::git::interceptor::ingest_turns_for_session(
            cfg,
            root,
            agent,
            sid,
            transcript_path.as_deref(),
        );
    }

    let snapshots = crate::git::turns::list_turn_snapshots(root);
    let tap_turns = crate::attribution::turn_store::read_all_turns(root);
    let now = chrono::Utc::now().timestamp();

    let mut session_refs: Vec<v2::SessionRef> = Vec::new();
    let mut coverage_tools: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut gap_files: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    let mut sorted: Vec<(&String, &String)> = relevant.iter().collect();
    sorted.sort();
    for (sid, agent) in sorted {
        let tool = crate::core::tool::normalize_source(agent).to_string();
        coverage_tools.insert(tool.clone());
        let suid = identity::session_uid(agent, sid);
        let state = sessions.iter().find(|s| &s.session_id == sid);

        // Home: the session's ORIGIN repo. Foreign sessions get a stub
        // with a pointer; origin sessions get the conversation layer here.
        let origin_root = state.and_then(|s| s.worktree.clone());
        let is_home = origin_root
            .as_deref()
            .is_none_or(|o| canon(o) == canon_root);
        let home_location = if is_home {
            None
        } else {
            origin_root.as_deref().map(v2::home_location_for)
        };

        // Lineage from explicit signals only: a tool-reported resume id at
        // session-start, or a parent whose subagent-start hook named this
        // session as the spawned child. Never inferred.
        let lineage = crate::core::identity::SessionLineage {
            resumed_from: state
                .and_then(|s| s.resumed_from.as_deref())
                .map(|prior| identity::session_uid(agent, prior)),
            compacted_from: None,
            parent_session_uid: sessions.iter().find_map(|p| {
                if &p.session_id == sid {
                    return None;
                }
                p.subagent_runs
                    .as_ref()?
                    .iter()
                    .find(|r| &r.agent_id == sid)
                    .map(|_| identity::session_uid(&p.agent, &p.session_id))
            }),
        };

        // turn_count must reflect the turns actually being persisted, not
        // just hook-state's counter: a MID-turn commit drains before the
        // counter is bumped, and the record must never undercount its own
        // stored turns. Counter-max on merge keeps this monotonic.
        let observed_turns = snapshots
            .iter()
            .filter(|t| &t.session_id == sid)
            .map(|t| t.turn_index + 1)
            .max()
            .unwrap_or(0);
        let turn_count = observed_turns.max(state.and_then(|s| s.turn_count).map_or(0, i64::from));

        let record = v2::SessionRecord {
            schema_version: v2::V2_SCHEMA_VERSION,
            session_uid: suid.clone(),
            native_session_ids: vec![sid.clone()],
            tool,
            model: state.and_then(|s| s.model.clone()),
            home_location: home_location.clone(),
            origin_repo_id: origin_root.as_deref().map(crate::project::id_for_root),
            repos_touched: vec![repo_id.clone()],
            lineage,
            turn_count,
            title: None,
            started_at: state.map_or(0, |s| s.started_at),
            updated_at: state.map_or(now, |s| s.updated_at),
            ended_at: state.and_then(|s| s.ended_at),
        };
        if let Err(e) = v2::write_provenance_session(root, &repo_id, &record) {
            tracing::warn!(%e, "v2 provenance session write failed");
            return false;
        }
        if is_home {
            if let Err(e) = v2::write_conversation_session(root, &record) {
                tracing::warn!(%e, "v2 conversation session write failed");
                return false;
            }
        }

        // Provenance turns: join git-backed turn snapshots (edit
        // evidence) with tap turns (token deltas) BY TIME. Tap turns
        // index per transcript ENTRY (each user message, each assistant
        // API call); snapshots index per oobo TURN (prompt → stop).
        // Different index spaces — and one oobo turn is usually MANY
        // billed API calls, so the turn's tokens are the SUM of every
        // assistant call inside its window.
        const JOIN_SLACK_SECS: i64 = 2;
        let ts_of = crate::attribution::turn_store::turn_ts_secs;
        let mut turn_uids = Vec::new();
        for snap in snapshots.iter().filter(|t| &t.session_id == sid) {
            let tuid = identity::turn_uid(&suid, sid, snap.turn_index);
            let wstart = snap.started_at.unwrap_or(snap.created_at) - JOIN_SLACK_SECS;
            let wend = snap.ended_at.unwrap_or(snap.created_at) + JOIN_SLACK_SECS;

            let calls: Vec<&crate::core::turn::Turn> = tap_turns
                .iter()
                .filter(|t| {
                    &t.session_id == sid
                        && t.role == crate::core::turn::TurnRole::Assistant
                        && ts_of(t).is_some_and(|ts| ts >= wstart && ts <= wend)
                })
                .collect();

            let mut tokens = crate::core::turn::TurnTokens::default();
            let mut model: Option<String> = None;
            let mut tool_names: Vec<String> = Vec::new();
            for call in &calls {
                tokens.accumulate(&call.tokens);
                if call.model.is_some() {
                    model.clone_from(&call.model);
                }
                if let Some(names) = &call.tool_names {
                    tool_names.extend(names.split(',').filter(|n| !n.is_empty()).map(String::from));
                }
            }
            if calls.is_empty() {
                // Timestamp-less taps: degrade to the index join (only
                // sound when the tap indexes per oobo turn).
                if let Some(t) = tap_turns.iter().find(|t| {
                    &t.session_id == sid
                        && t.turn_index == snap.turn_index
                        && t.role == crate::core::turn::TurnRole::Assistant
                        && ts_of(t).is_none()
                }) {
                    tokens = t.tokens;
                    model.clone_from(&t.model);
                    tool_names = t
                        .tool_names
                        .clone()
                        .map(|names| names.split(',').map(str::to_string).collect())
                        .unwrap_or_default();
                }
            }

            // The human prompt that directed this turn: the latest user
            // entry at/before the end of the turn's window.
            let trigger = tap_turns
                .iter()
                .filter(|t| {
                    &t.session_id == sid
                        && t.role == crate::core::turn::TurnRole::User
                        && t.message_preview.is_some()
                        && ts_of(t).is_some_and(|ts| ts <= wend)
                })
                .max_by_key(|t| (ts_of(t), t.turn_index))
                .and_then(|t| t.message_preview.clone());
            let capture_gap = snap.files.iter().any(|f| f.capture_gap);
            for f in snap.files.iter().filter(|f| f.capture_gap) {
                gap_files.insert(f.path.clone());
            }

            let turn = v2::TurnRecord {
                schema_version: v2::V2_SCHEMA_VERSION,
                turn_uid: tuid.clone(),
                session_uid: suid.clone(),
                turn_index: snap.turn_index,
                native_turn_index: Some(snap.turn_index),
                source: snap.source.clone(),
                model,
                trigger,
                started_at: snap.started_at,
                ended_at: snap.ended_at,
                tokens,
                tool_names,
                capture_gap,
            };
            let edits = v2::TurnEdits {
                files: snap.files.clone(),
            };
            if let Err(e) = v2::write_provenance_turn(root, &repo_id, &turn, &edits) {
                tracing::warn!(%e, "v2 provenance turn write failed");
                return false;
            }

            // Conversation layer: full turn memory (transcript slice +
            // tool calls) written ONCE, at the session's home repo only,
            // gated behind transparency mode like v1 transcripts.
            // Content-addressed-idempotent: an already-stored turn
            // index is never overwritten.
            if is_home
                && outcome.anchor.transparency_mode == crate::core::anchor::TransparencyMode::On
                && (snap.memory.transcript.is_some() || !snap.memory.tool_calls.is_empty())
            {
                let transcript_json = serde_json::json!({
                    "schema_version": v2::V2_SCHEMA_VERSION,
                    "turn_uid": tuid,
                    "session_uid": suid,
                    "turn_index": snap.turn_index,
                    "native_transcript_path": snap.memory.transcript_path,
                    "transcript": snap.memory.transcript,
                })
                .to_string();
                let tool_calls_json =
                    serde_json::to_string(&snap.memory.tool_calls).unwrap_or_else(|_| "[]".into());
                if let Err(e) = v2::write_conversation_turn(
                    root,
                    &suid,
                    snap.turn_index,
                    &transcript_json,
                    &tool_calls_json,
                ) {
                    tracing::warn!(%e, "v2 conversation turn write failed");
                }
            }
            turn_uids.push(tuid);
        }

        session_refs.push(v2::SessionRef {
            session_uid: suid,
            home_location,
            turn_uids,
        });
    }

    let coverage = v2::CoverageManifest {
        tools: coverage_tools.into_iter().collect(),
        hook_events_seen: Vec::new(),
        capture_gap_files: gap_files.into_iter().collect(),
        recorded_at: now,
    };

    let timeline = outcome
        .anchor
        .file_interactions
        .as_ref()
        .filter(|i| !i.is_empty())
        .and_then(|interactions| {
            crate::git::orphan::build_timeline_json(
                root,
                &outcome.anchor,
                &outcome.session_links,
                interactions,
            )
            .ok()
        });

    let record = v2::AnchorRecord {
        anchor: outcome.anchor.clone(),
        session_refs,
        session_links: outcome.session_links.clone(),
        coverage: Some(coverage),
    };
    if let Err(e) = v2::write_anchor(root, &repo_id, &record, timeline.as_deref()) {
        if matches!(e, crate::error::CliError::SecretBlocked) {
            tracing::warn!("v2 anchor blocked by secret detection — skipping (not retryable)");
            record_anchor_drop(root, &entry.sha, "secret_detected");
        } else {
            tracing::warn!(%e, "v2 anchor write failed — will retry");
            return false;
        }
    }
    crate::git::anchor_cache::invalidate(root);
    true
}

fn canon(path: &str) -> String {
    std::fs::canonicalize(path)
        .map_or_else(|_| path.to_string(), |p| p.to_string_lossy().to_string())
}

/// Record that an anchor was intentionally dropped so `oobo status` can
/// surface it to the user. Writes to `~/.oobo/state/last-drop.json`.
fn record_anchor_drop(project_root: &str, sha: &str, reason: &str) {
    let dir = crate::paths::oobo_home().join("state");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let entry = serde_json::json!({
        "commit": sha,
        "reason": reason,
        "repo": project_root,
        "timestamp": chrono::Utc::now().timestamp(),
    });
    let _ = std::fs::write(dir.join("last-drop.json"), entry.to_string());
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    #[test]
    #[serial]
    fn test_record_anchor_drop_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("OOBO_HOME", tmp.path());

        record_anchor_drop("/home/user/project", "abc123def456", "secret_detected");

        let path = tmp.path().join("state").join("last-drop.json");
        assert!(path.exists(), "last-drop.json should be created");

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["commit"], "abc123def456");
        assert_eq!(parsed["reason"], "secret_detected");
        assert_eq!(parsed["repo"], "/home/user/project");
        assert!(parsed["timestamp"].is_number());
    }

    #[test]
    #[serial]
    fn test_record_anchor_drop_overwrites_previous() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("OOBO_HOME", tmp.path());

        record_anchor_drop("/repo1", "sha-first", "secret_detected");
        record_anchor_drop("/repo2", "sha-second", "secret_detected");

        let path = tmp.path().join("state").join("last-drop.json");
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["commit"], "sha-second",
            "last drop should overwrite previous"
        );
        assert_eq!(parsed["repo"], "/repo2");
    }
}
