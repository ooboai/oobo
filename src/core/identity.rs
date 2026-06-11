//! Attribution v2 identity primitives.
//!
//! Stable, deterministic identities for sessions and turns. These are the
//! keys everything in the v2 model hangs off: the orphan-branch store,
//! cross-repo pointers, token accounting, and provenance records.
//!
//! Design rules (see `.docs/attribution-v2-causal-provenance.md`):
//!
//! - Tool-native session ids are **seeds, not identities**. They are
//!   meaningless without the tool that minted them, and resume/compaction
//!   mints a new native id for the same logical conversation.
//! - `session_uid` is a UUIDv5 over `"{tool}:{native_session_id}"` in the
//!   oobo namespace — any machine or writer capturing the same native
//!   session converges on the same uid, which is what makes
//!   "store once, update in place" safe across concurrent writers.
//! - `turn_uid` derives from the **segment-native** pair
//!   (`native_session_id`, `native_turn_index`), never from the continuous
//!   display index. Two machines resuming the same session could both mint
//!   "turn 42" of the merged timeline; the segment-native pair cannot
//!   collide. The continuous index is computed at read time and is never
//!   used as identity.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Fixed oobo identity namespace: `uuidv5(NAMESPACE_DNS, "oobo.ai")`.
/// Hardcoded so it is a true constant; guarded by a test below.
pub const OOBO_NAMESPACE: Uuid = Uuid::from_bytes([
    0x2d, 0x7c, 0x53, 0xfa, 0xba, 0x39, 0x53, 0x79, 0xa8, 0x8f, 0xc1, 0x0e, 0xfd, 0x8c, 0x64, 0x50,
]);

/// Deterministic global session identity.
///
/// `tool` is normalized via [`crate::core::tool::normalize_source`] so that
/// e.g. Cursor's various agent-mode names all map to the same identity.
pub fn session_uid(tool: &str, native_session_id: &str) -> String {
    let tool = crate::core::tool::normalize_source(tool);
    let name = format!("{tool}:{native_session_id}");
    Uuid::new_v5(&OOBO_NAMESPACE, name.as_bytes()).to_string()
}

/// Deterministic global turn identity, collision-free across resume seams.
///
/// Keyed by the *segment* (the native session that actually produced the
/// turn) and the turn's index within that segment — NOT by the continuous
/// index of the merged "one long session" timeline.
pub fn turn_uid(session_uid: &str, native_session_id: &str, native_turn_index: i64) -> String {
    let name = format!("{session_uid}:{native_session_id}:{native_turn_index}");
    Uuid::new_v5(&OOBO_NAMESPACE, name.as_bytes()).to_string()
}

/// Two-character shard prefix for orphan-branch path fan-out
/// (`sessions/<uid[0:2]>/<uid>/`), mirroring `.git/objects` layout.
pub fn shard_prefix(uid: &str) -> &str {
    if uid.len() >= 2 {
        &uid[..2]
    } else {
        uid
    }
}

/// Lineage links recorded on a session stub. These collapse tool-side
/// session splits (resume, compaction) into one logical session, and tie
/// subagents to their parents.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLineage {
    /// `session_uid` of the session this one resumed. Continuation turns
    /// append to the chain root; this link is how the root is found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
    /// `session_uid` of the session this one was compacted from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compacted_from: Option<String>,
    /// `session_uid` of the parent session when this is a subagent.
    /// Subagents stay separate session objects (distinct actors), unlike
    /// resume/compaction continuations which merge into the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_uid: Option<String>,
}

impl SessionLineage {
    pub fn is_empty(&self) -> bool {
        self.resumed_from.is_none()
            && self.compacted_from.is_none()
            && self.parent_session_uid.is_none()
    }

    /// Whether this session is a continuation that merges into a chain
    /// root (resume or compaction), as opposed to a standalone session or
    /// a subagent.
    pub fn is_continuation(&self) -> bool {
        self.resumed_from.is_some() || self.compacted_from.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_matches_dns_derivation() {
        assert_eq!(
            OOBO_NAMESPACE,
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"oobo.ai")
        );
    }

    #[test]
    fn session_uid_is_deterministic() {
        let a = session_uid("claude", "abc-123");
        let b = session_uid("claude", "abc-123");
        assert_eq!(a, b);
        assert_eq!(a.len(), 36);
    }

    #[test]
    fn session_uid_differs_across_tools_and_ids() {
        let same_id_different_tool = (
            session_uid("claude", "abc-123"),
            session_uid("gemini", "abc-123"),
        );
        assert_ne!(same_id_different_tool.0, same_id_different_tool.1);

        let same_tool_different_id = (
            session_uid("claude", "abc-123"),
            session_uid("claude", "abc-124"),
        );
        assert_ne!(same_tool_different_id.0, same_tool_different_id.1);
    }

    #[test]
    fn session_uid_normalizes_cursor_agent_aliases() {
        // Cursor hooks report various agent-mode names; all must converge
        // on one identity for the same native session.
        let a = session_uid("cursor", "s1");
        let b = session_uid("composer", "s1");
        assert_eq!(a, b);
    }

    #[test]
    fn turn_uid_no_collision_across_resume_seams() {
        // "One long session": segment A (original) and segment B (resume)
        // both contribute turns. If identity used the continuous display
        // index, both machines could mint "turn 3". The segment-native
        // derivation must keep them distinct.
        let suid = session_uid("claude", "root-session");
        let from_segment_a = turn_uid(&suid, "root-session", 3);
        let from_segment_b = turn_uid(&suid, "resumed-session", 3);
        assert_ne!(from_segment_a, from_segment_b);

        // ... while staying deterministic within a segment.
        assert_eq!(from_segment_a, turn_uid(&suid, "root-session", 3));
    }

    #[test]
    fn shard_prefix_basics() {
        assert_eq!(shard_prefix("ab12cd"), "ab");
        assert_eq!(shard_prefix("a"), "a");
        assert_eq!(shard_prefix(""), "");
    }

    #[test]
    fn lineage_flags() {
        let mut l = SessionLineage::default();
        assert!(l.is_empty());
        assert!(!l.is_continuation());

        l.resumed_from = Some("x".into());
        assert!(!l.is_empty());
        assert!(l.is_continuation());

        let sub = SessionLineage {
            parent_session_uid: Some("p".into()),
            ..Default::default()
        };
        assert!(!sub.is_continuation());
        assert!(!sub.is_empty());
    }
}
