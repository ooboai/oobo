//! Automatic, non-user-facing backfill runner.
//!
//! The new data model (v11 turns + v12 subagent inference) is built
//! by re-reading the tool-native transcripts on disk. A user should
//! *never* have to run a "rebuild" command  --  the rebuild happens:
//!
//! 1. **On every `oobo update`**  --  the post-update hook always runs
//!    a fresh full backfill so new token accounting and inference
//!    logic take effect immediately.
//!
//! 2. **Opportunistically, once per schema bump**  --  migration v12
//!    arms a `backfill_pending` flag in `oobo_state`. The first
//!    `oobo` invocation afterward runs the backfill (silently, in
//!    the background conceptually  --  today synchronously, cheaply)
//!    and clears the flag. Subsequent invocations do nothing.
//!
//! This module is the single source of both triggers.

use crate::config::Config;
use crate::db::Db;

const PENDING_KEY: &str = "backfill_pending";

/// Aggregate outcome of a multi-project backfill.
#[derive(Debug, Clone, Default)]
pub struct AggregateReport {
    pub projects_attempted: usize,
    pub projects_succeeded: usize,
    pub sessions_scanned: usize,
    pub turns_emitted: u64,
    pub contributions_written: usize,
    pub subagents_inferred: usize,
    pub subagents_proposed: usize,
    pub warnings: usize,
    pub failures: Vec<(String, String)>,
}

/// Run a full backfill across every known project. Never fails the
/// process as a whole  --  per-project errors are captured in
/// [`AggregateReport::failures`] so the caller can report them
/// without aborting the calling flow (update, first-run trigger).
pub fn backfill_all_projects(db: &mut Db, cfg: &Config) -> AggregateReport {
    let mut report = AggregateReport::default();

    let projects = match db.list_projects() {
        Ok(list) => list,
        Err(e) => {
            report.failures.push(("<list>".into(), e));
            return report;
        }
    };

    report.projects_attempted = projects.len();

    for project in projects {
        match super::backfill::backfill_project(db, cfg, &project.id, &project.path) {
            Ok(r) => {
                report.projects_succeeded += 1;
                report.sessions_scanned += r.sessions_scanned;
                report.turns_emitted += r.tap_summary.turns_emitted;
                report.contributions_written += r.contributions_written;
                report.subagents_inferred += r.inference.applied;
                report.subagents_proposed += r.inference.proposed;
                report.warnings += r.tap_summary.warnings.len();
            }
            Err(e) => report.failures.push((project.id.clone(), e)),
        }
    }

    report
}

/// If the "backfill pending" flag is set in `oobo_state`, run a
/// full backfill and clear the flag. This is the opportunistic
/// first-run trigger that fires exactly once after migration v12.
///
/// Returns `Ok(Some(report))` when a backfill actually ran, and
/// `Ok(None)` when the flag was absent. Errors are swallowed into
/// the report's `failures` field, so the caller never has to worry
/// about aborting a user-facing command on backfill hiccups.
pub fn backfill_if_pending(db: &mut Db, cfg: &Config) -> Option<AggregateReport> {
    if db.state_get(PENDING_KEY).as_deref() != Some("1") {
        return None;
    }

    let report = backfill_all_projects(db, cfg);

    // Clear the flag even if some projects failed  --  leaving it set
    // would retrigger on every invocation and spam the user. The
    // admin can always re-run `oobo update --post-update` to force
    // another pass.
    if let Err(e) = db.state_clear(PENDING_KEY) {
        // Not fatal  --  but surface to the caller via the report so
        // we don't silently lose this.
        let mut r = report;
        r.failures.push(("<clear-flag>".into(), e));
        return Some(r);
    }

    Some(report)
}

/// Force a backfill across every known project and clear any
/// pending flag. Called from `oobo update --post-update`.
pub fn backfill_force_all(db: &mut Db, cfg: &Config) -> AggregateReport {
    let report = backfill_all_projects(db, cfg);
    let _ = db.state_clear(PENDING_KEY);
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_flag_not_set_returns_none() {
        let mut db = Db::open_in_memory().unwrap();
        // The v12 migration arms the flag on fresh DBs; clear it
        // so this test verifies the "no flag → noop" path.
        db.state_clear(PENDING_KEY).unwrap();
        let cfg = Config::default();
        assert!(backfill_if_pending(&mut db, &cfg).is_none());
    }

    #[test]
    fn pending_flag_set_triggers_and_clears() {
        let mut db = Db::open_in_memory().unwrap();
        db.state_set(PENDING_KEY, "1").unwrap();
        assert_eq!(db.state_get(PENDING_KEY).as_deref(), Some("1"));

        let mut cfg = Config::default();
        cfg.claude.enabled = false;
        cfg.cursor.enabled = false;
        cfg.codex.enabled = false;
        cfg.opencode.enabled = false;

        let r = backfill_if_pending(&mut db, &cfg).unwrap();
        assert_eq!(r.projects_attempted, 0);
        assert_eq!(db.state_get(PENDING_KEY), None);
    }

    #[test]
    fn v12_migration_arms_the_flag() {
        let db = Db::open_in_memory().unwrap();
        // Migrations ran during open_in_memory; check state.
        assert_eq!(db.state_get(PENDING_KEY).as_deref(), Some("1"));
    }

    #[test]
    fn force_all_clears_flag_even_if_already_set() {
        let mut db = Db::open_in_memory().unwrap();
        db.state_set(PENDING_KEY, "1").unwrap();
        let cfg = Config::default();
        let _ = backfill_force_all(&mut db, &cfg);
        assert_eq!(db.state_get(PENDING_KEY), None);
    }
}
