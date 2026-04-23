//! Anchor contributions: the DELTA of work a session contributed to a
//! specific commit.
//!
//! A [`Contribution`] ties a commit (anchor) to a contiguous window of
//! turns `[first_turn_index, last_turn_index]` in a session. The
//! denormalized token/cost/tool fields are sums of **just those
//! turns**, not the session's cumulative totals. This is what makes
//! per-project and per-commit rollups numerically correct:
//!
//! `SUM(contribution deltas for session S) == session S total`
//!
//! regardless of how many commits S contributed to.
//!
//! Contributions are written by the L3 attribution pass, which is the
//! only code that decides "which turns belong to which anchor". That
//! logic is in [`crate::attribution`]; this file is the pure data
//! type.

use super::turn::TurnTokens;
use serde::{Deserialize, Serialize};

/// How the link between session and anchor was established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkType {
    /// The tool itself (or a hook) told us "session X produced commit Y".
    /// Strongest signal; use whenever available.
    Explicit,
    /// Derived from time / file / parent-turn heuristics in the
    /// attribution pass. Tunable.
    Inferred,
    /// Manual annotation via `oobo` commands.
    Manual,
}

impl LinkType {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkType::Explicit => "explicit",
            LinkType::Inferred => "inferred",
            LinkType::Manual => "manual",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "explicit" => LinkType::Explicit,
            "manual" => LinkType::Manual,
            _ => LinkType::Inferred,
        }
    }
}

/// A single (commit, session) contribution record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contribution {
    pub commit_hash: String,
    pub session_id: String,
    pub source: String,

    pub link_type: LinkType,

    /// Inclusive window of session turns counted in this contribution.
    /// `first_turn_index <= last_turn_index` always holds.
    pub first_turn_index: i64,
    pub last_turn_index: i64,

    /// Sum of per-turn tokens over `[first_turn_index, last_turn_index]`.
    pub tokens: TurnTokens,

    pub cost_usd: Option<f64>,
    pub tool_call_count: Option<i64>,
    pub duration_secs: Option<i64>,

    /// True if the session is a subagent of another session that also
    /// contributed to this commit. When the UI shows a collapsed view
    /// it should sum `is_subagent = 1` rows under their parent.
    pub is_subagent: bool,

    pub parent_session_id: Option<String>,
    pub parent_source: Option<String>,

    /// Human-readable label for the subagent role (e.g. "task",
    /// "explore", "general-purpose"). Free-form; sourced from the
    /// tool's native naming when present.
    pub subagent_kind: Option<String>,
}

impl Contribution {
    /// Invariant check: the window is well-formed. Returns an error
    /// string useful for propagating into migration / attribution
    /// errors, not a `Result<()>` with a bespoke error type.
    pub fn validate(&self) -> Result<(), String> {
        if self.first_turn_index < 0 || self.last_turn_index < 0 {
            return Err(format!(
                "contribution {}.{} has negative turn index",
                self.commit_hash, self.session_id
            ));
        }
        if self.first_turn_index > self.last_turn_index {
            return Err(format!(
                "contribution {}.{} has inverted window [{}, {}]",
                self.commit_hash, self.session_id, self.first_turn_index, self.last_turn_index
            ));
        }
        Ok(())
    }

    /// Number of turns spanned by this contribution.
    pub fn turn_count(&self) -> i64 {
        (self.last_turn_index - self.first_turn_index) + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(first: i64, last: i64) -> Contribution {
        Contribution {
            commit_hash: "c1".into(),
            session_id: "s1".into(),
            source: "claude".into(),
            link_type: LinkType::Inferred,
            first_turn_index: first,
            last_turn_index: last,
            tokens: TurnTokens::default(),
            cost_usd: None,
            tool_call_count: None,
            duration_secs: None,
            is_subagent: false,
            parent_session_id: None,
            parent_source: None,
            subagent_kind: None,
        }
    }

    #[test]
    fn validate_rejects_negative() {
        assert!(fixture(-1, 0).validate().is_err());
    }

    #[test]
    fn validate_rejects_inverted_window() {
        assert!(fixture(5, 2).validate().is_err());
    }

    #[test]
    fn validate_accepts_single_turn_window() {
        let c = fixture(3, 3);
        assert!(c.validate().is_ok());
        assert_eq!(c.turn_count(), 1);
    }

    #[test]
    fn turn_count_is_inclusive() {
        assert_eq!(fixture(0, 0).turn_count(), 1);
        assert_eq!(fixture(0, 9).turn_count(), 10);
    }

    #[test]
    fn link_type_round_trips() {
        for lt in [LinkType::Explicit, LinkType::Inferred, LinkType::Manual] {
            assert_eq!(LinkType::parse(lt.as_str()), lt);
        }
        // Unknown values fall back to inferred (safe default).
        assert_eq!(LinkType::parse("bogus"), LinkType::Inferred);
    }
}
