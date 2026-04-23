//! End-to-end backfill: discover sessions → run taps → infer
//! subagent links → rebuild contributions.
//!
//! This is the operational entry point that turns the new pipeline
//! from "schema + code" into "data the readers can show". Every call
//! is idempotent thanks to upsert semantics all the way down.
//!
//! All five first-class sources are wired up here:
//! - **Claude** — JSONL per session (+ explicit subagents directory)
//! - **Cursor** — SQLite state.vscdb (`bubbleId:` rows)
//! - **Codex** — JSONL rollouts in `~/.codex/sessions/`
//! - **OpenCode** — SQLite `opencode.db` (modern and legacy schemas)
//!
//! Adding a new source = adding one entry to [`ingest_sessions_for_source`].

use std::path::PathBuf;

use crate::config::Config;
use crate::db::Db;
use crate::taps::{
    claude::ClaudeTurnTap, codex::CodexTurnTap, cursor::CursorTurnTap, opencode::OpenCodeTurnTap,
    TapArtifact, TapSummary, TurnSink, TurnTap,
};

/// Aggregate outcome of a backfill run, suitable for display.
#[derive(Debug, Clone, Default)]
pub struct BackfillReport {
    pub sessions_scanned: usize,
    pub tap_summary: TapSummary,
    pub contributions_written: usize,
    /// M4 inference pass results.
    pub inference: super::inference_runner::InferenceReport,
    /// Per-source breakdown for observability. Key = `Source`.
    pub per_source: std::collections::BTreeMap<String, SourceReport>,
}

#[derive(Debug, Clone, Default)]
pub struct SourceReport {
    pub sessions_scanned: usize,
    pub summary: TapSummary,
}

/// Full backfill for one project across every enabled source.
///
/// 1. For each enabled source, enumerate sessions for this project,
///    run its tap on each, and upsert turns + subagent links.
/// 2. Run the M4 inference engine to fill implicit parent/child
///    links for sessions the taps couldn't resolve on their own.
/// 3. Recompute `anchor_contributions` from the fresh `turns` table.
///
/// Safe to call at any time.
pub fn backfill_project(
    db: &mut Db,
    cfg: &Config,
    project_id: &str,
    project_path: &str,
) -> Result<BackfillReport, String> {
    let mut report = BackfillReport::default();

    let combined = db
        .with_turn_sink(Some(project_id.to_string()), |sink| {
            let mut acc = TapSummary::default();
            let mut total_sessions = 0usize;
            let mut per_source: std::collections::BTreeMap<String, SourceReport> =
                std::collections::BTreeMap::new();

            for (source, scanned, summary) in ingest_all_sources(cfg, project_path, sink) {
                total_sessions += scanned;
                acc = acc.merged(summary.clone());
                per_source.insert(
                    source.to_string(),
                    SourceReport {
                        sessions_scanned: scanned,
                        summary,
                    },
                );
            }
            (acc, total_sessions, per_source)
        })
        .map_err(|e| format!("backfill: {e}"))?;

    let (summary, sessions_scanned, per_source) = combined;
    report.sessions_scanned = sessions_scanned;
    report.tap_summary = summary;
    report.per_source = per_source;

    // M4 — infer subagent parent/child links *before* attribution so
    // contributions inherit `is_subagent` from the freshly populated
    // sessions columns. The order is:
    //   taps → inference → attribution
    // Tap-provided explicit links are preserved by the inference
    // pass (it only fills rows where `sessions.parent_session_id IS NULL`).
    let inference = super::inference_runner::infer_subagents_for_project(db, project_id)?;
    report.inference = inference;

    let n = super::runner::rebuild_contributions_for_project(db, project_id)?;
    report.contributions_written = n;
    Ok(report)
}

/// Dispatch: runs every enabled tap for this project, emitting turns
/// into `sink`. Returns one `(source, sessions_scanned, summary)`
/// tuple per source.
fn ingest_all_sources(
    cfg: &Config,
    project_path: &str,
    sink: &mut dyn TurnSink,
) -> Vec<(&'static str, usize, TapSummary)> {
    let mut out = Vec::new();

    if cfg.claude.enabled {
        out.push(ingest_claude(project_path, sink));
    }
    if cfg.cursor.enabled {
        out.push(ingest_cursor(project_path, sink));
    }
    if cfg.codex.enabled {
        out.push(ingest_codex(project_path, sink));
    }
    if cfg.opencode.enabled {
        out.push(ingest_opencode(project_path, sink));
    }

    out
}

fn ingest_claude(project_path: &str, sink: &mut dyn TurnSink) -> (&'static str, usize, TapSummary) {
    use crate::tools::claude;
    use crate::tools::claude::transcript as claude_transcript;

    let sessions = claude::sessions_for_project(project_path).unwrap_or_default();
    let tap = ClaudeTurnTap;
    let mut acc = TapSummary::default();

    for sess in &sessions {
        let primary =
            match claude_transcript::find_transcript_path(&sess.project_path, &sess.session_id) {
                Some(p) => p,
                None => continue,
            };

        let subagent_pairs: Vec<(String, PathBuf)> =
            claude_transcript::find_subagent_transcripts(&sess.project_path, &sess.session_id);

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
            Err(e) => acc
                .warnings
                .push(format!("claude session {} ingest failed: {e}", sess.session_id)),
        }
    }

    ("claude", sessions.len(), acc)
}

fn ingest_cursor(project_path: &str, sink: &mut dyn TurnSink) -> (&'static str, usize, TapSummary) {
    use crate::tools::cursor;

    let sessions = cursor::sessions_for_project(project_path).unwrap_or_default();
    let tap = CursorTurnTap;
    let mut acc = TapSummary::default();

    for sess in &sessions {
        match tap.ingest_session(&sess.session_id, TapArtifact::SelfLookup, sink) {
            Ok(s) => acc = acc.merged(s),
            Err(e) => acc
                .warnings
                .push(format!("cursor session {} ingest failed: {e}", sess.session_id)),
        }
    }

    ("cursor", sessions.len(), acc)
}

fn ingest_codex(project_path: &str, sink: &mut dyn TurnSink) -> (&'static str, usize, TapSummary) {
    use crate::tools::codex::{self, transcript as codex_transcript};

    let sessions = codex::sessions_for_project(project_path).unwrap_or_default();
    let tap = CodexTurnTap;
    let mut acc = TapSummary::default();

    for sess in &sessions {
        let primary =
            match codex_transcript::find_transcript_path(&sess.project_path, &sess.session_id) {
                Some(p) => p,
                None => continue,
            };
        match tap.ingest_session(&sess.session_id, TapArtifact::File(&primary), sink) {
            Ok(s) => acc = acc.merged(s),
            Err(e) => acc
                .warnings
                .push(format!("codex session {} ingest failed: {e}", sess.session_id)),
        }
    }

    ("codex", sessions.len(), acc)
}

fn ingest_opencode(
    project_path: &str,
    sink: &mut dyn TurnSink,
) -> (&'static str, usize, TapSummary) {
    use crate::tools::opencode;

    let db_path = match opencode::find_db_path() {
        Some(p) => p,
        None => return ("opencode", 0, TapSummary::default()),
    };

    let sessions = opencode::sessions_for_project(project_path).unwrap_or_default();
    let tap = OpenCodeTurnTap;
    let mut acc = TapSummary::default();

    for sess in &sessions {
        match tap.ingest_session(&sess.session_id, TapArtifact::File(&db_path), sink) {
            Ok(s) => acc = acc.merged(s),
            Err(e) => acc.warnings.push(format!(
                "opencode session {} ingest failed: {e}",
                sess.session_id
            )),
        }
    }

    ("opencode", sessions.len(), acc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backfill_with_all_sources_disabled_is_noop() {
        let mut db = Db::open_in_memory().unwrap();
        let mut cfg = Config::default();
        cfg.claude.enabled = false;
        cfg.cursor.enabled = false;
        cfg.codex.enabled = false;
        cfg.opencode.enabled = false;

        let report = backfill_project(&mut db, &cfg, "r:t/p", "/tmp/p").unwrap();
        assert_eq!(report.sessions_scanned, 0);
        assert_eq!(report.contributions_written, 0);
        assert!(report.per_source.is_empty());
    }
}
