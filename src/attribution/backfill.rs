//! End-to-end backfill: discover sessions → run tap → rebuild
//! contributions.
//!
//! This is the operational entry point that turns the new pipeline
//! from "schema + code" into "data the readers can show". Every call
//! is idempotent thanks to upsert semantics all the way down.

use std::path::PathBuf;

use crate::config::Config;
use crate::db::Db;
use crate::taps::{claude::ClaudeTurnTap, TapArtifact, TapSummary, TurnTap};
use crate::tools::claude;
use crate::tools::claude::transcript as claude_transcript;

/// Aggregate outcome of a backfill run, suitable for display.
#[derive(Debug, Clone, Default)]
pub struct BackfillReport {
    pub sessions_scanned: usize,
    pub tap_summary: TapSummary,
    pub contributions_written: usize,
    /// M4 inference pass results.
    pub inference: super::inference_runner::InferenceReport,
}

/// Full backfill for one project:
///
/// 1. Enumerate every Claude session for this project path.
/// 2. For each, run [`ClaudeTurnTap`] with its primary JSONL plus
///    any subagent transcripts on disk.
/// 3. Recompute `anchor_contributions` for the project from the
///    fresh `turns` table.
///
/// Safe to call at any time. Returns a report suitable for display.
pub fn backfill_project(
    db: &mut Db,
    cfg: &Config,
    project_id: &str,
    project_path: &str,
) -> Result<BackfillReport, String> {
    let mut report = BackfillReport::default();

    if !cfg.claude.enabled {
        return Ok(report);
    }

    let sessions = claude::sessions_for_project(project_path).unwrap_or_default();
    report.sessions_scanned = sessions.len();

    db.with_turn_sink(Some(project_id.to_string()), |sink| {
        let tap = ClaudeTurnTap;
        let mut acc = TapSummary::default();

        for sess in &sessions {
            let primary = match claude_transcript::find_transcript_path(
                &sess.project_path,
                &sess.session_id,
            ) {
                Some(p) => p,
                None => continue,
            };

            let subagent_pairs: Vec<(String, PathBuf)> =
                claude_transcript::find_subagent_transcripts(
                    &sess.project_path,
                    &sess.session_id,
                );

            let artifact = if subagent_pairs.is_empty() {
                TapArtifact::File(&primary)
            } else {
                TapArtifact::FileWithSubagents {
                    primary: &primary,
                    subagents: &subagent_pairs,
                }
            };

            match tap.ingest_session(&sess.session_id, artifact, sink) {
                Ok(s) => acc = acc.merged(s),
                Err(e) => acc.warnings.push(format!(
                    "session {} ingest failed: {e}",
                    sess.session_id
                )),
            }
        }

        acc
    })
    .and_then(|summary| {
        report.tap_summary = summary;

        // M4 — infer subagent parent/child links *before* attribution
        // so contributions inherit `is_subagent` from the freshly
        // populated sessions columns. The order is:
        //   taps → inference → attribution
        // Tap-provided explicit links are preserved by the inference
        // pass; attribution reads `sessions.parent_*` as authoritative.
        let inference = super::inference_runner::infer_subagents_for_project(
            db, project_id,
        )?;
        report.inference = inference;

        let n = super::runner::rebuild_contributions_for_project(db, project_id)?;
        report.contributions_written = n;
        Ok(report)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backfill_with_claude_disabled_is_noop() {
        let mut db = Db::open_in_memory().unwrap();
        let mut cfg = Config::default();
        cfg.claude.enabled = false;

        let report = backfill_project(&mut db, &cfg, "r:t/p", "/tmp/p").unwrap();
        assert_eq!(report.sessions_scanned, 0);
        assert_eq!(report.contributions_written, 0);
    }
}
