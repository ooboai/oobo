//! Evidence gathering + trigger classification for the provenance engine.
//!
//! Two evidence sources, merged and deduplicated:
//! 1. **The v2 store** (durable) — the anchor's session refs lead to
//!    provenance turns whose `edits.json` carries per-file pre/post
//!    blobs. Survives session-state GC, clones with the repo.
//! 2. **Live capture state** (fresh) — session edit chains and turn
//!    snapshots not yet (or never) anchored. Covers querying provenance
//!    for a commit the worker hasn't drained yet.
//!
//! Trigger classification per edit, in precedence order:
//! - a subagent window covered the edit → [`Trigger::Subagent`]
//! - the turn has a captured prompt → [`Trigger::HumanDirected`]
//! - the session has captured turns at all → [`Trigger::AgentAutonomous`]
//! - otherwise → [`Trigger::Unknown`]

use std::collections::HashSet;

use super::{
    compute_file_provenance, is_null_blob, EditInput, FileProvenance, GitBlobSource, Trigger,
};
use crate::git::orphan::v2;

/// Provenance for one file at one commit: cache → compute → cache.
pub fn file_provenance(repo_root: &str, commit: &str, path: &str) -> Option<FileProvenance> {
    let sha = resolve_commit(repo_root, commit)?;
    if let Some(cached) = super::cache::read(repo_root, &sha, path) {
        return Some(cached);
    }

    let baseline_blob = blob_at(repo_root, &format!("{sha}~1"), path).unwrap_or_default();
    let committed_blob = blob_at(repo_root, &sha, path)?;

    let edits = gather_edits(repo_root, &sha, path);
    let blobs = GitBlobSource { repo_root };
    let provenance =
        compute_file_provenance(&blobs, &sha, path, &baseline_blob, &committed_blob, edits);

    super::cache::write(repo_root, &provenance);
    Some(provenance)
}

/// All captured edit events for `(commit, path)` from both evidence
/// sources, deduplicated by `(session, pre, post)`.
fn gather_edits(repo_root: &str, sha: &str, path: &str) -> Vec<EditInput> {
    let mut out: Vec<EditInput> = Vec::new();
    let mut seen: HashSet<(String, String, String)> = HashSet::new();
    let mut push = |e: EditInput, out: &mut Vec<EditInput>| {
        if seen.insert((
            e.session_id.clone(),
            e.pre_blob.clone(),
            e.post_blob.clone(),
        )) {
            out.push(e);
        }
    };

    // Source 1: the v2 store, via the anchor's session refs.
    let repo_id = crate::project::id_for_root(repo_root);
    if let Some(record) = v2::read_anchor(repo_root, &repo_id, sha) {
        for sref in &record.session_refs {
            let session = v2::read_provenance_session(repo_root, &repo_id, &sref.session_uid);
            let native_id = session
                .as_ref()
                .and_then(|s| s.native_session_ids.first().cloned())
                .unwrap_or_else(|| sref.session_uid.clone());
            for (turn, edits) in v2::list_provenance_turns(repo_root, &repo_id, &sref.session_uid) {
                let trigger = match &turn.trigger {
                    Some(prompt) => Trigger::HumanDirected {
                        prompt: Some(prompt.clone()),
                    },
                    None => Trigger::AgentAutonomous,
                };
                for f in &edits.files {
                    if f.path != path {
                        continue;
                    }
                    let (Some(pre), Some(post)) = (&f.pre_blob, &f.post_blob) else {
                        continue;
                    };
                    push(
                        EditInput {
                            session_id: native_id.clone(),
                            agent: turn.source.clone(),
                            turn_index: Some(turn.turn_index),
                            seq: turn.turn_index,
                            timestamp_us: turn.ended_at.or(turn.started_at).unwrap_or(0),
                            pre_blob: pre.clone(),
                            post_blob: post.clone(),
                            tool_name: None,
                            trigger: trigger.clone(),
                        },
                        &mut out,
                    );
                }
            }
        }
    }

    // Source 2: live capture state (pre-drain commits, un-anchored work).
    let sessions = crate::hooks::store::list_for_project(repo_root);
    for pe in crate::attribution::claim::pending_for_repo(repo_root) {
        if pe.path != path {
            continue;
        }
        let trigger = classify_live(repo_root, &sessions, &pe);
        push(
            EditInput {
                session_id: pe.session_id,
                agent: pe.agent,
                turn_index: pe.turn_index,
                seq: pe.seq,
                timestamp_us: pe.timestamp_us,
                pre_blob: pe.pre_blob,
                post_blob: pe.post_blob,
                tool_name: pe.tool_name,
                trigger,
            },
            &mut out,
        );
    }

    out
}

/// Classify a live (hook-captured) edit.
fn classify_live(
    repo_root: &str,
    sessions: &[crate::hooks::state::ActiveSession],
    pe: &crate::attribution::claim::PendingEdit,
) -> Trigger {
    let state = sessions.iter().find(|s| s.session_id == pe.session_id);

    // Subagent window covering the edit timestamp wins: the edit was
    // made by a delegated agent, intent lives in the parent turn.
    if let Some(s) = state {
        let edit_secs = pe.timestamp_us / 1_000_000;
        if let Some(runs) = &s.subagent_runs {
            for run in runs {
                let ended = run.ended_at.unwrap_or(i64::MAX);
                if edit_secs >= run.started_at && edit_secs <= ended {
                    return Trigger::Subagent {
                        agent_type: Some(run.agent_type.clone()),
                    };
                }
            }
        }
    }

    // Captured prompt for this turn → human-directed. Tap turns index
    // per transcript ENTRY, not per oobo turn, so the match is by time:
    // the latest human prompt at/before the edit. Timestamp-less taps
    // degrade to the index comparison.
    let ts_of = crate::attribution::turn_store::turn_ts_secs;
    let taps = crate::attribution::turn_store::read_all_turns(repo_root);
    let edit_secs = pe.timestamp_us / 1_000_000;
    let prompt = taps
        .iter()
        .filter(|t| {
            t.session_id == pe.session_id
                && t.role == crate::core::turn::TurnRole::User
                && t.message_preview.is_some()
                && ts_of(t).is_some_and(|ts| ts <= edit_secs + 2)
        })
        .max_by_key(|t| (ts_of(t), t.turn_index))
        .and_then(|t| t.message_preview.clone())
        .or_else(|| {
            let idx = pe.turn_index?;
            taps.iter()
                .filter(|t| {
                    t.session_id == pe.session_id
                        && t.role == crate::core::turn::TurnRole::User
                        && ts_of(t).is_none()
                        && t.turn_index <= idx
                })
                .max_by_key(|t| t.turn_index)
                .and_then(|t| t.message_preview.clone())
        });
    if let Some(prompt) = prompt {
        return Trigger::HumanDirected {
            prompt: Some(prompt),
        };
    }

    // We know the session was an agent loop (it has turns) but found no
    // fresh prompt → autonomous. No turn data at all → unknown.
    if taps.iter().any(|t| t.session_id == pe.session_id) || state.is_some() {
        Trigger::AgentAutonomous
    } else {
        Trigger::Unknown
    }
}

fn resolve_commit(repo_root: &str, commitish: &str) -> Option<String> {
    let probe = format!("{commitish}^{{commit}}");
    git(repo_root, &["rev-parse", "--verify", "--quiet", &probe])
}

/// Blob id of `path` at `rev` (None when absent at that rev).
fn blob_at(repo_root: &str, rev: &str, path: &str) -> Option<String> {
    let spec = format!("{rev}:{path}");
    let blob = git(repo_root, &["rev-parse", "--verify", "--quiet", &spec])?;
    if is_null_blob(&blob) {
        None
    } else {
        Some(blob)
    }
}

fn git(repo_root: &str, args: &[&str]) -> Option<String> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let output = std::process::Command::new(git)
        .args(args)
        .current_dir(repo_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_QUARANTINE_PATH")
        .output()
        .ok()?;
    if output.status.success() {
        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribution::claim::PendingEdit;
    use crate::hooks::state::test_support::mk_session_with;
    use crate::hooks::state::SubagentRun;

    fn pe(session: &str, ts_us: i64, turn: Option<i64>) -> PendingEdit {
        PendingEdit {
            session_id: session.into(),
            agent: "claude".into(),
            path: "f.rs".into(),
            pre_blob: "a".into(),
            post_blob: "b".into(),
            seq: 1,
            timestamp_us: ts_us,
            turn_index: turn,
            tool_name: None,
        }
    }

    #[test]
    fn subagent_window_wins_classification() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();

        let mut s = mk_session_with("sid-1", root, None, None);
        s.subagent_runs = Some(vec![SubagentRun {
            agent_id: "task-1".into(),
            agent_type: "explore".into(),
            started_at: 1_000,
            ended_at: Some(2_000),
        }]);
        let sessions = vec![s];

        // Edit at t=1500s: inside the subagent window.
        let t = classify_live(root, &sessions, &pe("sid-1", 1_500_000_000, Some(1)));
        assert_eq!(
            t,
            Trigger::Subagent {
                agent_type: Some("explore".into())
            }
        );

        // Edit at t=5000s: outside → falls through to autonomous
        // (session state exists, no captured prompt).
        let t = classify_live(root, &sessions, &pe("sid-1", 5_000_000_000, Some(1)));
        assert_eq!(t, Trigger::AgentAutonomous);
    }

    #[test]
    fn unknown_when_no_session_context_at_all() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let t = classify_live(root, &[], &pe("ghost", 1_000_000, None));
        assert_eq!(t, Trigger::Unknown);
    }

    #[test]
    fn open_ended_subagent_window_still_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();

        let mut s = mk_session_with("sid-1", root, None, None);
        s.subagent_runs = Some(vec![SubagentRun {
            agent_id: "task-1".into(),
            agent_type: "generalPurpose".into(),
            started_at: 1_000,
            ended_at: None, // still running
        }]);
        let t = classify_live(root, &[s], &pe("sid-1", 9_999_000_000, Some(2)));
        assert!(matches!(t, Trigger::Subagent { .. }));
    }
}
