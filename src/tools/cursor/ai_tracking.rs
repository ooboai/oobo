#![allow(dead_code)]

use std::path::PathBuf;

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

/// Ingest scored commits from Cursor's ai-code-tracking.db.
/// No-op after DB removal — ingestion will be rebuilt on the orphan branch model.
pub fn ingest_scored_commits(_force: bool) -> Result<(usize, usize), String> {
    Ok((0, 0))
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
