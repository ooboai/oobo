//! `oobo _rebuild` — internal, hidden command.
//!
//! Re-derives the v11 `turns` and `anchor_contributions` tables from the
//! native tool transcripts on disk. This is the operational counterpart
//! of the "proper rebuild" migration path: when a user wants to see the
//! new, accurate token numbers, they (or a maintainer) run this once.
//!
//! Scope:
//! - By default, rebuilds the **current project** (the repo we're inside).
//! - With `--all`, iterates every known project in `projects`.
//!
//! This command is intentionally hidden from `--help` because the eventual
//! UX is fully automatic (opportunistic rebuild from the TUI picker, and
//! incremental upsert from hooks). Exposing it today gives maintainers a
//! deterministic button to press while that automation is still landing.

use crate::attribution::backfill::{backfill_project, BackfillReport};
use crate::cli::OutputMode;
use crate::config::Config;
use crate::db::Db;

pub fn run(cfg: &Config, all: bool, mode: OutputMode) -> Result<i32, String> {
    let mut db = Db::open()?;

    let targets: Vec<(String, String)> = if all {
        db.list_projects()?
            .into_iter()
            .map(|p| (p.id, p.path))
            .collect()
    } else {
        let root = match crate::git::proxy::project_root(cfg) {
            Some(r) => r,
            None => {
                emit_not_in_repo(mode);
                return Ok(1);
            }
        };
        let project_id = crate::project::ensure_stable(&db, &root)?;
        vec![(project_id, root)]
    };

    if targets.is_empty() {
        emit_no_projects(mode);
        return Ok(0);
    }

    let mut grand = BackfillReport::default();
    let mut failures: Vec<(String, String)> = Vec::new();

    for (pid, path) in &targets {
        match backfill_project(&mut db, cfg, pid, path) {
            Ok(report) => {
                emit_project_done(pid, path, &report, mode);
                grand.sessions_scanned += report.sessions_scanned;
                grand.contributions_written += report.contributions_written;
                grand.tap_summary.turns_emitted += report.tap_summary.turns_emitted;
                grand.tap_summary.turns_skipped += report.tap_summary.turns_skipped;
                grand.tap_summary.warnings.extend(report.tap_summary.warnings);
            }
            Err(e) => failures.push((pid.clone(), e)),
        }
    }

    emit_summary(&grand, &failures, targets.len(), mode);

    if failures.is_empty() {
        Ok(0)
    } else {
        Ok(1)
    }
}

fn emit_not_in_repo(mode: OutputMode) {
    match mode {
        OutputMode::Json => {
            println!(r#"{{"ok":false,"error":"not in a git repo; pass --all to rebuild every project"}}"#);
        }
        _ => {
            eprintln!(
                "oobo _rebuild: not inside a git repo. Pass --all to rebuild every known project."
            );
        }
    }
}

fn emit_no_projects(mode: OutputMode) {
    match mode {
        OutputMode::Json => println!(r#"{{"ok":true,"projects":0}}"#),
        _ => eprintln!("oobo _rebuild: no projects recorded yet."),
    }
}

fn emit_project_done(project_id: &str, path: &str, r: &BackfillReport, mode: OutputMode) {
    match mode {
        OutputMode::Json => {
            println!(
                r#"{{"project_id":"{}","path":"{}","sessions":{},"turns":{},"contributions":{}}}"#,
                escape(project_id),
                escape(path),
                r.sessions_scanned,
                r.tap_summary.turns_emitted,
                r.contributions_written,
            );
        }
        OutputMode::Agent => {
            println!(
                "{}\t{}\tsessions={}\tturns={}\tcontribs={}",
                project_id,
                path,
                r.sessions_scanned,
                r.tap_summary.turns_emitted,
                r.contributions_written,
            );
        }
        OutputMode::Tui => {
            println!(
                "  {} ({} sessions → {} turns → {} contributions)",
                path,
                r.sessions_scanned,
                r.tap_summary.turns_emitted,
                r.contributions_written,
            );
        }
    }
}

fn emit_summary(
    grand: &BackfillReport,
    failures: &[(String, String)],
    project_count: usize,
    mode: OutputMode,
) {
    match mode {
        OutputMode::Json => {
            println!(
                r#"{{"ok":{},"projects":{},"sessions":{},"turns":{},"contributions":{},"failures":{}}}"#,
                failures.is_empty(),
                project_count,
                grand.sessions_scanned,
                grand.tap_summary.turns_emitted,
                grand.contributions_written,
                failures.len(),
            );
        }
        OutputMode::Agent => {
            println!(
                "total\tprojects={}\tsessions={}\tturns={}\tcontribs={}\tfailures={}",
                project_count,
                grand.sessions_scanned,
                grand.tap_summary.turns_emitted,
                grand.contributions_written,
                failures.len(),
            );
            for (pid, err) in failures {
                eprintln!("fail\t{pid}\t{err}");
            }
        }
        OutputMode::Tui => {
            println!();
            println!(
                "rebuilt {} project(s): {} sessions, {} turns, {} contributions.",
                project_count,
                grand.sessions_scanned,
                grand.tap_summary.turns_emitted,
                grand.contributions_written,
            );
            for (pid, err) in failures {
                eprintln!("  failed: {pid}: {err}");
            }
            if !grand.tap_summary.warnings.is_empty() {
                eprintln!("  warnings: {}", grand.tap_summary.warnings.len());
            }
        }
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
