//! Run attribution for a single anchor at commit time.
//!
//! Reads cached turns, computes contribution windows, and enriches
//! session links with per-window token/cost deltas.
//!
//! The attribution cursor is persisted across commits
//! (`.oobo/cache/attribution-cursor.json`): once a turn window has been
//! attributed to a commit, later commits only receive the *new* turns.
//! Without the persisted cursor every commit would re-claim the whole
//! session — the token double-counting bug attribution v2 removes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::attribution::{compute_windows, AttrAnchor, AttrTurn, PriorCursor};
use crate::core::anchor::SessionLink;
use crate::core::turn::TurnRole;

/// Enrich session links with attribution data from cached turns.
///
/// Reads all cached turns for the project, runs the attribution
/// algorithm resuming from the persisted cursor, updates session links
/// with delta token counts, then advances the cursor.
pub fn enrich_session_links(
    project_root: &str,
    commit_hash: &str,
    committed_at: i64,
    session_links: &mut [SessionLink],
) {
    let all_turns = crate::attribution::turn_store::read_all_turns(project_root);
    if all_turns.is_empty() {
        return;
    }

    let attr_turns: Vec<AttrTurn> = all_turns
        .iter()
        .filter(|t| t.role == TurnRole::Assistant)
        .map(|t| AttrTurn {
            session_id: t.session_id.clone(),
            source: t.source.clone(),
            turn_index: t.turn_index,
            started_at: t.started_at.map(|ts| {
                if ts < 1_000_000_000_000 {
                    ts * 1000
                } else {
                    ts
                }
            }),
            tokens: t.tokens,
            cost_usd: t.cost_usd,
            tool_call_count: t.tool_call_count,
            duration_ms: t.ended_at.and_then(|e| {
                t.started_at.map(|s| {
                    let diff = e - s;
                    if diff < 1_000_000_000 {
                        diff * 1000
                    } else {
                        diff
                    }
                })
            }),
        })
        .collect();

    let anchor = AttrAnchor {
        commit_hash: commit_hash.to_string(),
        committed_at_ms: if committed_at < 1_000_000_000_000 {
            committed_at * 1000
        } else {
            committed_at
        },
    };

    let cursor = load_cursor(project_root);
    let contributions = compute_windows(&[anchor], &attr_turns, cursor.clone());

    for contribution in &contributions {
        if let Some(link) = session_links
            .iter_mut()
            .find(|l| l.session_id == contribution.session_id)
        {
            if let Some(input) = contribution.tokens.input {
                link.input_tokens = Some(input as u64);
            }
            if let Some(output) = contribution.tokens.output {
                link.output_tokens = Some(output as u64);
            }
            if let Some(cache_read) = contribution.tokens.cache_read {
                link.cache_read_tokens = Some(cache_read as u64);
            }
            if let Some(cache_creation) = contribution.tokens.cache_creation {
                link.cache_creation_tokens = Some(cache_creation as u64);
            }
            if let Some(tool_calls) = contribution.tool_call_count {
                link.tool_calls = Some(tool_calls as u32);
            }
            if let Some(dur) = contribution.duration_secs {
                link.duration_secs = Some(dur as u64);
            }
        }
    }

    // Advance the cursor past everything attributed to this commit so the
    // next commit only claims new turns.
    let mut advanced = cursor;
    for contribution in &contributions {
        let key = (contribution.session_id.clone(), contribution.source.clone());
        let entry = advanced.entry(key).or_insert(-1);
        *entry = (*entry).max(contribution.last_turn_index);
    }
    save_cursor(project_root, &advanced);
}

#[derive(Serialize, Deserialize)]
struct CursorEntry {
    session_id: String,
    source: String,
    last_turn_index: i64,
}

fn cursor_path(project_root: &str) -> PathBuf {
    Path::new(project_root)
        .join(".oobo")
        .join("cache")
        .join("attribution-cursor.json")
}

fn load_cursor(project_root: &str) -> PriorCursor {
    let Ok(content) = std::fs::read_to_string(cursor_path(project_root)) else {
        return PriorCursor::new();
    };
    let entries: Vec<CursorEntry> = serde_json::from_str(&content).unwrap_or_default();
    entries
        .into_iter()
        .map(|e| ((e.session_id, e.source), e.last_turn_index))
        .collect()
}

/// Best-effort atomic write (temp file + rename). A lost cursor update
/// degrades to re-attribution of a window — never to data loss.
fn save_cursor(project_root: &str, cursor: &PriorCursor) {
    let mut entries: Vec<CursorEntry> = cursor
        .iter()
        .map(|((session_id, source), last)| CursorEntry {
            session_id: session_id.clone(),
            source: source.clone(),
            last_turn_index: *last,
        })
        .collect();
    entries.sort_by(|a, b| {
        (a.session_id.as_str(), a.source.as_str()).cmp(&(b.session_id.as_str(), b.source.as_str()))
    });

    let Ok(json) = serde_json::to_string(&entries) else {
        return;
    };
    let path = cursor_path(project_root);
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::turn::{Turn, TurnRole, TurnTokens};

    fn make_turn(session_id: &str, idx: i64, started_at_ms: i64, output: i64) -> Turn {
        Turn {
            id: Turn::deterministic_id("claude", session_id, idx),
            session_id: session_id.to_string(),
            source: "claude".to_string(),
            turn_index: idx,
            role: TurnRole::Assistant,
            started_at: Some(started_at_ms),
            ended_at: Some(started_at_ms + 1000),
            model: None,
            tokens: TurnTokens {
                output: Some(output),
                ..Default::default()
            },
            cost_usd: None,
            tool_call_count: 0,
            thinking_ms: None,
            message_preview: None,
            raw_ref: None,
            tool_names: None,
        }
    }

    fn make_link(session_id: &str) -> SessionLink {
        serde_json::from_value(serde_json::json!({
            "session_id": session_id,
            "agent": "claude",
            "link_type": "inferred",
        }))
        .unwrap()
    }

    #[test]
    fn consecutive_commits_do_not_double_count() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_str().unwrap();

        // Session with two turns before commit 1.
        crate::attribution::turn_store::write_turns(
            root,
            "claude",
            "s1",
            &[
                make_turn("s1", 0, 100_000, 10),
                make_turn("s1", 1, 200_000, 20),
            ],
        )
        .unwrap();

        let mut links1 = vec![make_link("s1")];
        enrich_session_links(root, "c1", 300_000, &mut links1);
        assert_eq!(links1[0].output_tokens, Some(30), "c1 claims turns 0-1");

        // Two more turns arrive, then commit 2.
        crate::attribution::turn_store::write_turns(
            root,
            "claude",
            "s1",
            &[
                make_turn("s1", 0, 100_000, 10),
                make_turn("s1", 1, 200_000, 20),
                make_turn("s1", 2, 400_000, 40),
                make_turn("s1", 3, 500_000, 80),
            ],
        )
        .unwrap();

        let mut links2 = vec![make_link("s1")];
        enrich_session_links(root, "c2", 600_000, &mut links2);
        assert_eq!(
            links2[0].output_tokens,
            Some(120),
            "c2 claims ONLY turns 2-3 — without the persisted cursor this would be 150"
        );

        // Invariant: sum of per-commit deltas == total session spend.
        let total = links1[0].output_tokens.unwrap() + links2[0].output_tokens.unwrap();
        assert_eq!(total, 150);
    }

    #[test]
    fn commit_with_no_new_turns_claims_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_str().unwrap();

        crate::attribution::turn_store::write_turns(
            root,
            "claude",
            "s1",
            &[make_turn("s1", 0, 100_000, 10)],
        )
        .unwrap();

        let mut links1 = vec![make_link("s1")];
        enrich_session_links(root, "c1", 200_000, &mut links1);
        assert_eq!(links1[0].output_tokens, Some(10));

        // Immediate second commit, no agent activity in between.
        let mut links2 = vec![make_link("s1")];
        enrich_session_links(root, "c2", 250_000, &mut links2);
        assert_eq!(
            links2[0].output_tokens, None,
            "no new turns → no tokens attributed"
        );
    }

    /// The token-partition invariant, three commits deep: every turn's
    /// spend lands on exactly one commit, and the per-commit deltas sum
    /// to the session total — no double-counting, no leakage.
    #[test]
    fn three_commits_partition_session_tokens_exactly_once() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_str().unwrap();

        let turns: Vec<Turn> = (0..6)
            .map(|i| make_turn("s1", i, 100_000 * (i + 1), 10 * (i + 1)))
            .collect();
        let session_total: u64 = turns.iter().map(|t| t.tokens.output.unwrap() as u64).sum();

        // Commit 1 after turns 0-1, commit 2 after 2-3, commit 3 after 4-5.
        let mut claimed_total = 0u64;
        for (commit_idx, visible) in [(0, 2usize), (1, 4), (2, 6)] {
            crate::attribution::turn_store::write_turns(root, "claude", "s1", &turns[..visible])
                .unwrap();
            let mut links = vec![make_link("s1")];
            let committed_at = 100_000 * visible as i64 + 50_000;
            enrich_session_links(root, &format!("c{commit_idx}"), committed_at, &mut links);
            claimed_total += links[0].output_tokens.unwrap_or(0);
        }

        assert_eq!(
            claimed_total, session_total,
            "sum of per-commit deltas must equal the session spend exactly"
        );

        // A fourth commit with no new turns claims nothing.
        let mut links = vec![make_link("s1")];
        enrich_session_links(root, "c3", 10_000_000, &mut links);
        assert_eq!(links[0].output_tokens, None);
    }

    #[test]
    fn cursor_round_trips_through_disk() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_str().unwrap();

        let mut cursor = PriorCursor::new();
        cursor.insert(("s1".into(), "claude".into()), 4);
        cursor.insert(("s2".into(), "codex".into()), 7);
        save_cursor(root, &cursor);

        let loaded = load_cursor(root);
        assert_eq!(loaded, cursor);
    }

    #[test]
    fn missing_cursor_file_is_empty_cursor() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_str().unwrap();
        assert!(load_cursor(root).is_empty());
    }
}
