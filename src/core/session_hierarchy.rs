//! Subagent hierarchy types.
//!
//! A session can be the child of another session when the parent
//! spawned it via a sub-agent mechanism (Claude's Task tool, Cursor's
//! background agents, Codex's plan / execute split, etc.).
//!
//! Modeling subagents as first-class child sessions (rather than as
//! flattened tool calls) means:
//!
//! 1. Their tokens roll up correctly into their own `v_session_totals`
//!    row, never double-counted into the parent.
//! 2. Anchor contributions can surface "this commit used a research
//!    subagent for 7k tokens" as a distinct line item.
//! 3. The TUI can draw a tree: parent turn → subagent → subagent turns.
//!
//! The hierarchy is stored on `sessions` (parent_session_id /
//! parent_source / parent_turn_id / subagent_kind) so that *any*
//! reader querying a session gets the full context in one join.

use serde::{Deserialize, Serialize};

/// A link from a child session up to the parent turn that spawned it.
///
/// When the L1 tap cannot produce an explicit link, M4 falls back to
/// heuristic inference (see [`InferenceSignal`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionHierarchy {
    pub session_id: String,
    pub source: String,

    pub parent_session_id: Option<String>,
    pub parent_source: Option<String>,

    /// The `turns.id` of the parent turn that fired the subagent
    /// (e.g. the parent turn whose tool_call_count > 0 with a Task
    /// invocation). Enables "this subagent produced 2.3k tokens while
    /// the parent was executing *this* turn" UI.
    pub parent_turn_id: Option<String>,

    /// Free-form role label ("task", "plan", "execute",
    /// "general-purpose", ...). Normalized labels are owned by the
    /// tool adapters, not core.
    pub subagent_kind: Option<String>,
}

impl SessionHierarchy {
    pub fn root(session_id: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            source: source.into(),
            parent_session_id: None,
            parent_source: None,
            parent_turn_id: None,
            subagent_kind: None,
        }
    }

    pub fn is_subagent(&self) -> bool {
        self.parent_session_id.is_some()
    }
}

/// Evidence used by the heuristic inference pass to decide whether a
/// session is a subagent and, if so, who its parent is.
///
/// The inference logic (in `src/attribution/subagents.rs`, landing in
/// M4) consumes a set of these signals per candidate and picks the
/// strongest explanation. Keeping the evidence explicit (rather than
/// baked into a big boolean) makes the rules testable and the
/// decisions auditable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InferenceSignal {
    /// Parent's turn included a `Task` tool call and the child session
    /// started within `window_secs` of that turn's completion.
    TaskToolTemporal { parent_turn_id: String, gap_secs: i64 },

    /// Child session's transcript path lives under the parent's
    /// session directory (tool-specific file layout signal).
    SubdirectoryArtifact { parent_session_id: String },

    /// Child session's first user turn content matches a known
    /// "subagent system prompt" template for this tool.
    TemplateMatch { template: String },

    /// Parent and child share a process-tree / pid relationship
    /// captured by a hook at the moment the subagent was spawned.
    ProcessTree { parent_turn_id: String },
}

impl InferenceSignal {
    /// Priority ordering: higher = stronger. When multiple signals
    /// agree on a parent, we prefer the strongest for attribution.
    pub fn strength(&self) -> i32 {
        match self {
            InferenceSignal::ProcessTree { .. } => 100,
            InferenceSignal::SubdirectoryArtifact { .. } => 80,
            InferenceSignal::TaskToolTemporal { .. } => 60,
            InferenceSignal::TemplateMatch { .. } => 40,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_session_is_not_subagent() {
        let r = SessionHierarchy::root("s1", "claude");
        assert!(!r.is_subagent());
    }

    #[test]
    fn hierarchy_with_parent_is_subagent() {
        let h = SessionHierarchy {
            session_id: "s2".into(),
            source: "claude".into(),
            parent_session_id: Some("s1".into()),
            parent_source: Some("claude".into()),
            parent_turn_id: Some("abcd1234".into()),
            subagent_kind: Some("task".into()),
        };
        assert!(h.is_subagent());
    }

    #[test]
    fn inference_signal_strength_ordered_correctly() {
        // Process-tree evidence beats everything else: a hook literally
        // saw the fork/exec, so it's the ground truth by definition.
        let strongest = InferenceSignal::ProcessTree {
            parent_turn_id: "t".into(),
        };
        let weakest = InferenceSignal::TemplateMatch {
            template: "x".into(),
        };
        assert!(strongest.strength() > weakest.strength());

        let temporal = InferenceSignal::TaskToolTemporal {
            parent_turn_id: "t".into(),
            gap_secs: 1,
        };
        let filesystem = InferenceSignal::SubdirectoryArtifact {
            parent_session_id: "p".into(),
        };
        assert!(filesystem.strength() > temporal.strength());
    }
}
