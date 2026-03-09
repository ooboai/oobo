#![allow(dead_code)]

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::ai_commits::AiCommitRow;
use crate::db::Db;

/// Path to Cursor's AI code tracking database.
fn tracking_db_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let path = home
        .join(".cursor")
        .join("ai-tracking")
        .join("ai-code-tracking.db");
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// Ingest scored commits from Cursor's ai-code-tracking.db into oobo's database.
/// Returns (ingested, skipped) count.
pub fn ingest_scored_commits(db: &Db, force: bool) -> Result<(usize, usize), String> {
    let path = match tracking_db_path() {
        Some(p) => p,
        None => return Ok((0, 0)),
    };

    let cursor_conn = crate::utils::open_db_readonly(&path)
        .map_err(|e| format!("cannot open Cursor ai-tracking db: {e}"))?;

    let existing_count = if force {
        0
    } else {
        db.ai_commit_count().unwrap_or(0)
    };

    let cursor_count: i64 = cursor_conn
        .query_row("SELECT COUNT(*) FROM scored_commits", [], |r| r.get(0))
        .map_err(|e| format!("cannot count scored_commits: {e}"))?;

    if !force && existing_count >= cursor_count {
        return Ok((0, existing_count as usize));
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let mut stmt = cursor_conn
        .prepare(
            "SELECT commitHash, branchName, linesAdded, linesDeleted,
                    tabLinesAdded, tabLinesDeleted,
                    composerLinesAdded, composerLinesDeleted,
                    humanLinesAdded, humanLinesDeleted,
                    commitMessage, commitDate, v2AiPercentage
             FROM scored_commits",
        )
        .map_err(|e| format!("cannot prepare scored_commits: {e}"))?;

    let mut ingested = 0usize;
    let mut skipped = 0usize;

    let rows = stmt
        .query_map([], |row| {
            let ai_pct_str: Option<String> = row.get(12)?;
            let ai_pct = ai_pct_str.and_then(|s| s.parse::<f64>().ok());

            Ok(AiCommitRow {
                commit_hash: row.get(0)?,
                branch_name: row.get(1)?,
                project_id: None,
                commit_message: row.get(10)?,
                commit_date: row.get(11)?,
                lines_added: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                lines_deleted: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                ai_lines_added: row.get::<_, Option<i64>>(6)?.unwrap_or(0),
                ai_lines_deleted: row.get::<_, Option<i64>>(7)?.unwrap_or(0),
                tab_lines_added: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                tab_lines_deleted: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                human_lines_added: row.get::<_, Option<i64>>(8)?.unwrap_or(0),
                human_lines_deleted: row.get::<_, Option<i64>>(9)?.unwrap_or(0),
                ai_percentage: ai_pct,
                source: "cursor".to_string(),
                ingested_at: now,
            })
        })
        .map_err(|e| format!("cannot query scored_commits: {e}"))?;

    for row_result in rows {
        let commit = row_result.map_err(|e| format!("row error: {e}"))?;

        if !force {
            let existing: i64 = db
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM ai_commits WHERE commit_hash = ?1 AND branch_name = ?2",
                    rusqlite::params![commit.commit_hash, commit.branch_name],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            if existing > 0 {
                skipped += 1;
                continue;
            }
        }

        db.upsert_ai_commit(&commit)?;
        ingested += 1;
    }

    Ok((ingested, skipped))
}

/// Summary of what's available in Cursor's tracking DB (for display).
pub fn tracking_db_stats() -> Option<TrackingDbInfo> {
    let path = tracking_db_path()?;
    let conn = crate::utils::open_db_readonly(&path).ok()?;

    let commit_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM scored_commits", [], |r| r.get(0))
        .ok()?;
    let hash_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM ai_code_hashes", [], |r| r.get(0))
        .ok()?;

    Some(TrackingDbInfo {
        commit_count,
        hash_count,
        db_path: path.display().to_string(),
    })
}

pub struct TrackingDbInfo {
    pub commit_count: i64,
    pub hash_count: i64,
    pub db_path: String,
}
