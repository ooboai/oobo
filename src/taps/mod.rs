//! L1 — Taps: tool-specific adapters that emit normalized [`Turn`]
//! streams.
//!
//! A tap's job is narrow and pure: given a handle to a session's
//! native artifact (a JSONL file, a SQLite DB row, a log), walk it
//! and emit a stream of [`Turn`] values to a [`TurnSink`]. Nothing
//! else.
//!
//! The four-layer pipeline:
//!
//! ```text
//!   L1 tap (this module)   →  Turn stream
//!   L2 store (src/turns)   →  idempotent upsert into `turns`
//!   L3 attribution         →  assigns turn windows to commits
//!   L4 readers             →  TUI, `oobo anchors`, API views
//! ```
//!
//! Adding a new tool = implementing one [`TurnTap`]. Readers do not
//! change.

pub mod claude;
pub mod codex;
pub mod cursor;
#[cfg(test)]
pub(crate) mod memory_sink;
pub mod opencode;

use crate::config::Config;
use crate::core::turn::Turn;

/// Source name for a tap. Matches `sessions.source` / `turns.source`.
pub type Source = &'static str;

/// Outcome of a single tap run over one session's native artifact.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TapSummary {
    pub turns_emitted: u64,
    pub turns_skipped: u64,
    pub subagent_links_emitted: u64,
    pub warnings: Vec<String>,
}

impl TapSummary {
    #[must_use]
    pub fn merged(mut self, other: TapSummary) -> Self {
        self.turns_emitted += other.turns_emitted;
        self.turns_skipped += other.turns_skipped;
        self.subagent_links_emitted += other.subagent_links_emitted;
        self.warnings.extend(other.warnings);
        self
    }
}

/// Errors a tap can raise. Taps should be forgiving: a malformed line
/// in the middle of a transcript must not fail the whole run. Use
/// [`TapSummary::warnings`] for recoverable issues; this error type
/// is reserved for unrecoverable ones (missing file, IO error, etc.).
#[derive(Debug, thiserror::Error)]
pub enum TapError {
    #[error("tap source missing: {0}")]
    SourceMissing(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

/// A link from a child session up to a parent turn in another session.
///
/// Emitted alongside the child's turns when the tap can determine the
/// hierarchy natively (e.g. Claude's `subagents/` directory layout).
/// When the tap can't, the L4 inference pass (M4) fills these in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentLink {
    pub child_session_id: String,
    pub child_source: String,
    pub parent_session_id: String,
    pub parent_source: String,
    /// `turns.id` of the parent turn that spawned the subagent, if
    /// identifiable; otherwise `None`. `None` still counts as an
    /// explicit session-level link, just without turn precision.
    pub parent_turn_id: Option<String>,
    pub subagent_kind: Option<String>,
}

/// L2's view of what L1 produces. The store implements this trait.
/// Kept minimal so taps compose with both the real DB store and the
/// in-memory test sink without friction.
pub trait TurnSink {
    fn accept_turn(&mut self, turn: Turn);
    fn accept_subagent_link(&mut self, link: SubagentLink);
}

/// A capture adapter for one AI tool. Implementations live alongside
/// the existing `tools::*` adapters and share their project / session
/// discovery helpers — tap is strictly "read the artifact, emit
/// turns", nothing about project scanning.
pub trait TurnTap {
    fn source(&self) -> Source;

    fn enabled(&self, cfg: &Config) -> bool;

    /// Ingest turns for one session. `session_id` is the adapter's
    /// native id (no prefixing). `artifact` is tool-specific (usually
    /// a filesystem path) located by the tool's existing discovery.
    fn ingest_session(
        &self,
        session_id: &str,
        artifact: TapArtifact<'_>,
        sink: &mut dyn TurnSink,
    ) -> Result<TapSummary, TapError>;
}

/// Opaque handle to the native artifact a tap should consume for a
/// given session. An enum (rather than `&Path`) so future taps can
/// carry richer context (e.g. a DB row id, a workspace hash) without
/// breaking the trait.
#[derive(Debug, Clone)]
pub enum TapArtifact<'a> {
    /// Single file on disk (e.g. Claude JSONL, Codex JSONL, OpenCode JSONL).
    File(&'a std::path::Path),
    /// Primary file plus a set of known subagent files (Claude's
    /// `subagents/` convention). The tap emits explicit subagent
    /// links for each.
    FileWithSubagents {
        primary: &'a std::path::Path,
        subagents: &'a [(String, std::path::PathBuf)],
    },
    /// Tap looks up the native artifact by `session_id` alone. Used
    /// for tools whose storage is a single global database shared
    /// across sessions (Cursor's `state.vscdb`). The tap owns the
    /// lookup policy; `session_id` is the complete identifier.
    SelfLookup,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_summary_merges_additively() {
        let a = TapSummary {
            turns_emitted: 3,
            turns_skipped: 1,
            subagent_links_emitted: 1,
            warnings: vec!["w1".into()],
        };
        let b = TapSummary {
            turns_emitted: 2,
            turns_skipped: 0,
            subagent_links_emitted: 0,
            warnings: vec!["w2".into()],
        };
        let merged = a.merged(b);
        assert_eq!(merged.turns_emitted, 5);
        assert_eq!(merged.turns_skipped, 1);
        assert_eq!(merged.subagent_links_emitted, 1);
        assert_eq!(merged.warnings, vec!["w1".to_string(), "w2".into()]);
    }
}
