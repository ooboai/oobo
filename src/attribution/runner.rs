//! Run attribution for a single anchor at commit time.
//!
//! Reads cached turns, computes contribution windows, and enriches
//! session links with per-window token/cost deltas.

use crate::attribution::{compute_windows, AttrAnchor, AttrTurn, PriorCursor};
use crate::core::anchor::SessionLink;
use crate::core::turn::TurnRole;

/// Enrich session links with attribution data from cached turns.
///
/// Reads all cached turns for the project, runs the attribution
/// algorithm, and updates session links with delta token counts
/// from the contribution windows.
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

    let cursor = PriorCursor::new();
    let contributions = compute_windows(&[anchor], &attr_turns, cursor);

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
}
