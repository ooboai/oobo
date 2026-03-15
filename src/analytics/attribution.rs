use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::ai_commits::AiCommitRow;
use crate::db::Db;

const SESSION_BUFFER_SECS: i64 = 300; // 5 minutes after session end

pub fn run_attribution(db: &Db, force: bool) -> Result<(usize, usize), String> {
    let projects = db.list_projects()?;
    let mut total_attributed = 0usize;
    let mut total_skipped = 0usize;

    let session_windows = build_session_windows(db)?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    for project in &projects {
        if project.path.is_empty() || !std::path::Path::new(&project.path).join(".git").exists() {
            continue;
        }

        let commits = parse_git_log(&project.path, 90)?;

        for commit in &commits {
            if !force {
                let exists: i64 = db
                    .conn
                    .query_row(
                        "SELECT COUNT(*) FROM ai_commits WHERE commit_hash = ?1",
                        rusqlite::params![commit.hash],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);

                if exists > 0 {
                    total_skipped += 1;
                    continue;
                }
            }

            // Prefer anchor data (real-time, file-level attribution) over correlation
            if let Some(row) = attribution_from_anchor(db, commit, &project.id, now) {
                db.upsert_ai_commit(&row)?;
                db.conn
                    .execute(
                        "UPDATE ai_commits SET commit_epoch = ?1 WHERE commit_hash = ?2 AND branch_name = ?3",
                        rusqlite::params![commit.epoch, commit.hash, row.branch_name],
                    )
                    .ok();
                total_attributed += 1;
                continue;
            }

            // Fallback: time-window correlation with session data
            let tool = find_active_session(&session_windows, &project.id, commit.epoch);

            if let Some(tool_name) = tool {
                let row = AiCommitRow {
                    commit_hash: commit.hash.clone(),
                    branch_name: commit
                        .branch
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                    project_id: Some(project.id.clone()),
                    commit_message: Some(commit.message.clone()),
                    commit_date: Some(commit.date_str.clone()),
                    lines_added: commit.lines_added,
                    lines_deleted: commit.lines_deleted,
                    ai_lines_added: commit.lines_added,
                    ai_lines_deleted: commit.lines_deleted,
                    tab_lines_added: 0,
                    tab_lines_deleted: 0,
                    human_lines_added: 0,
                    human_lines_deleted: 0,
                    ai_percentage: Some(100.0),
                    source: format!("correlation:{tool_name}"),
                    ingested_at: now,
                };

                db.upsert_ai_commit(&row)?;

                db.conn
                    .execute(
                        "UPDATE ai_commits SET commit_epoch = ?1 WHERE commit_hash = ?2 AND branch_name = ?3",
                        rusqlite::params![commit.epoch, commit.hash, row.branch_name],
                    )
                    .ok();

                total_attributed += 1;
            } else {
                let row = AiCommitRow {
                    commit_hash: commit.hash.clone(),
                    branch_name: commit
                        .branch
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                    project_id: Some(project.id.clone()),
                    commit_message: Some(commit.message.clone()),
                    commit_date: Some(commit.date_str.clone()),
                    lines_added: commit.lines_added,
                    lines_deleted: commit.lines_deleted,
                    ai_lines_added: 0,
                    ai_lines_deleted: 0,
                    tab_lines_added: 0,
                    tab_lines_deleted: 0,
                    human_lines_added: commit.lines_added,
                    human_lines_deleted: commit.lines_deleted,
                    ai_percentage: Some(0.0),
                    source: "correlation:human".to_string(),
                    ingested_at: now,
                };

                db.upsert_ai_commit(&row)?;

                db.conn
                    .execute(
                        "UPDATE ai_commits SET commit_epoch = ?1 WHERE commit_hash = ?2 AND branch_name = ?3",
                        rusqlite::params![commit.epoch, commit.hash, row.branch_name],
                    )
                    .ok();

                total_skipped += 1;
            }
        }
    }

    Ok((total_attributed, total_skipped))
}

/// Extract attribution data from a stored anchor (real-time, file-level).
/// Returns None if no anchor exists for this commit.
fn attribution_from_anchor(
    db: &Db,
    commit: &GitCommit,
    project_id: &str,
    now: i64,
) -> Option<AiCommitRow> {
    let anchor_json: String = db
        .conn
        .query_row(
            "SELECT data FROM anchors WHERE commit_hash = ?1",
            rusqlite::params![commit.hash],
            |r| r.get(0),
        )
        .ok()?;

    let anchor: crate::core::anchor::Anchor = serde_json::from_str(&anchor_json).ok()?;

    let ai_added = anchor.ai_added as i64;
    let ai_deleted = anchor.ai_deleted as i64;
    let human_added = anchor.human_added as i64;
    let human_deleted = anchor.human_deleted as i64;

    // Only use anchor data if it has file-level attribution (ai + human > 0 or file_changes present)
    if anchor.file_changes.is_empty() && ai_added == 0 && human_added == 0 {
        return None;
    }

    let source = if !anchor.session_ids.is_empty() {
        let agents: Vec<&str> = anchor
            .contributors
            .iter()
            .filter(|c| c.role == crate::core::anchor::ContributorRole::Agent)
            .map(|c| c.name.as_str())
            .collect();
        if agents.is_empty() {
            "anchor:assisted".to_string()
        } else {
            format!("anchor:{}", agents.join("+"))
        }
    } else {
        "anchor:human".to_string()
    };

    Some(AiCommitRow {
        commit_hash: commit.hash.clone(),
        branch_name: anchor.branch,
        project_id: Some(project_id.to_string()),
        commit_message: Some(anchor.message),
        commit_date: Some(commit.date_str.clone()),
        lines_added: anchor.added as i64,
        lines_deleted: anchor.deleted as i64,
        ai_lines_added: ai_added,
        ai_lines_deleted: ai_deleted,
        tab_lines_added: 0,
        tab_lines_deleted: 0,
        human_lines_added: human_added,
        human_lines_deleted: human_deleted,
        ai_percentage: anchor.ai_percentage,
        source,
        ingested_at: now,
    })
}

struct SessionWindow {
    project_id: String,
    source: String,
    start_epoch: i64,
    end_epoch: i64,
}

fn build_session_windows(db: &Db) -> Result<Vec<SessionWindow>, String> {
    let mut stmt = db
        .conn
        .prepare(
            "SELECT id, source, project_id, created_at, updated_at
             FROM sessions
             WHERE created_at IS NOT NULL",
        )
        .map_err(|e| format!("cannot query sessions for attribution: {e}"))?;

    let rows = stmt
        .query_map([], |row| {
            let source: String = row.get(1)?;
            let project_id: String = row.get(2)?;
            let created: Option<i64> = row.get(3)?;
            let updated: Option<i64> = row.get(4)?;
            Ok((source, project_id, created, updated))
        })
        .map_err(|e| format!("cannot fetch sessions: {e}"))?;

    let mut windows = Vec::new();
    for r in rows {
        let (source, project_id, created, updated) = r.map_err(|e| format!("row: {e}"))?;

        let created = match created {
            Some(c) => c,
            None => continue,
        };

        let start = normalize_epoch(created);
        let end = normalize_epoch(updated.unwrap_or(created)) + SESSION_BUFFER_SECS;

        if end <= start {
            continue;
        }

        windows.push(SessionWindow {
            project_id,
            source,
            start_epoch: start,
            end_epoch: end,
        });
    }

    windows.sort_by_key(|w| w.start_epoch);
    Ok(windows)
}

fn normalize_epoch(ts: i64) -> i64 {
    crate::utils::to_epoch_secs(ts)
}

fn find_active_session(
    windows: &[SessionWindow],
    project_id: &str,
    commit_epoch: i64,
) -> Option<String> {
    for w in windows {
        if w.project_id == project_id
            && commit_epoch >= w.start_epoch
            && commit_epoch <= w.end_epoch
        {
            let tool = if w.source.contains(':') {
                w.source.split(':').next().unwrap_or(&w.source)
            } else {
                &w.source
            };
            return Some(tool.to_string());
        }
    }
    None
}

struct GitCommit {
    hash: String,
    epoch: i64,
    message: String,
    date_str: String,
    branch: Option<String>,
    lines_added: i64,
    lines_deleted: i64,
}

fn parse_git_log(project_path: &str, days: u32) -> Result<Vec<GitCommit>, String> {
    let since = format!("--since={days} days ago");
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let output = Command::new(git)
        .args([
            "-C",
            project_path,
            "log",
            "--first-parent",
            &since,
            "--format=%H|%at|%s",
            "--numstat",
        ])
        .output()
        .map_err(|e| format!("git log failed in {project_path}: {e}"))?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut commits = Vec::new();
    let mut current: Option<GitCommit> = None;

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.splitn(3, '|').collect();
        if parts.len() == 3
            && parts[0].len() == 40
            && parts[0].chars().all(|c| c.is_ascii_hexdigit())
        {
            if let Some(c) = current.take() {
                commits.push(c);
            }
            let epoch: i64 = parts[1].parse().unwrap_or(0);
            current = Some(GitCommit {
                hash: parts[0].to_string(),
                epoch,
                message: parts[2].to_string(),
                date_str: epoch_to_date_string(epoch),
                branch: None,
                lines_added: 0,
                lines_deleted: 0,
            });
        } else if let Some(ref mut c) = current {
            let stat_parts: Vec<&str> = line.split('\t').collect();
            if stat_parts.len() >= 2 {
                let added: i64 = stat_parts[0].parse().unwrap_or(0);
                let deleted: i64 = stat_parts[1].parse().unwrap_or(0);
                c.lines_added += added;
                c.lines_deleted += deleted;
            }
        }
    }

    if let Some(c) = current {
        commits.push(c);
    }

    let branch = current_branch(project_path);
    for c in &mut commits {
        c.branch = branch.clone();
    }

    Ok(commits)
}

fn current_branch(project_path: &str) -> Option<String> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let output = Command::new(git)
        .args(["-C", project_path, "rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;

    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !branch.is_empty() && branch != "HEAD" {
            return Some(branch);
        }
    }
    None
}

fn epoch_to_date_string(epoch: i64) -> String {
    chrono::DateTime::from_timestamp(epoch, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_epoch() {
        assert_eq!(normalize_epoch(1709568000), 1709568000);
        assert_eq!(normalize_epoch(1709568000000), 1709568000);
    }

    #[test]
    fn test_find_active_session() {
        let windows = vec![
            SessionWindow {
                project_id: "proj1".to_string(),
                source: "claude".to_string(),
                start_epoch: 1000,
                end_epoch: 2000,
            },
            SessionWindow {
                project_id: "proj1".to_string(),
                source: "cursor".to_string(),
                start_epoch: 3000,
                end_epoch: 4000,
            },
        ];

        assert_eq!(
            find_active_session(&windows, "proj1", 1500),
            Some("claude".to_string())
        );
        assert_eq!(
            find_active_session(&windows, "proj1", 3500),
            Some("cursor".to_string())
        );
        assert_eq!(find_active_session(&windows, "proj1", 2500), None);
        assert_eq!(find_active_session(&windows, "proj2", 1500), None);
    }

    #[test]
    fn test_epoch_to_date_string() {
        let s = epoch_to_date_string(1709568000);
        assert!(!s.is_empty());
        assert!(s.starts_with("2024-03"));
    }

    #[test]
    fn test_find_active_session_colon_source() {
        let windows = vec![SessionWindow {
            project_id: "proj1".to_string(),
            source: "cursor:composer".to_string(),
            start_epoch: 1000,
            end_epoch: 2000,
        }];
        assert_eq!(
            find_active_session(&windows, "proj1", 1500),
            Some("cursor".to_string())
        );
    }

    #[test]
    fn test_find_active_session_empty_windows() {
        assert_eq!(find_active_session(&[], "proj1", 1500), None);
    }

    #[test]
    fn test_find_active_session_boundary_epochs() {
        let windows = vec![SessionWindow {
            project_id: "p".to_string(),
            source: "claude".to_string(),
            start_epoch: 100,
            end_epoch: 200,
        }];

        assert_eq!(
            find_active_session(&windows, "p", 100),
            Some("claude".to_string())
        );
        assert_eq!(
            find_active_session(&windows, "p", 200),
            Some("claude".to_string())
        );
        assert_eq!(find_active_session(&windows, "p", 99), None);
        assert_eq!(find_active_session(&windows, "p", 201), None);
    }

    #[test]
    fn test_find_active_session_first_match_wins() {
        let windows = vec![
            SessionWindow {
                project_id: "p".to_string(),
                source: "cursor".to_string(),
                start_epoch: 1000,
                end_epoch: 3000,
            },
            SessionWindow {
                project_id: "p".to_string(),
                source: "claude".to_string(),
                start_epoch: 2000,
                end_epoch: 4000,
            },
        ];
        assert_eq!(
            find_active_session(&windows, "p", 2500),
            Some("cursor".to_string())
        );
    }

    #[test]
    fn test_normalize_epoch_boundary() {
        assert_eq!(normalize_epoch(1_000_000_000_001), 1_000_000_000);
        // 1e12 is now correctly classified as milliseconds
        assert_eq!(normalize_epoch(1_000_000_000_000), 1_000_000_000);
        assert_eq!(normalize_epoch(0), 0);
    }

    #[test]
    fn test_build_session_windows_from_db() {
        use crate::db::projects::ProjectRow;
        use crate::db::sessions::SessionRow;
        use crate::db::Db;

        let db = Db::open_in_memory().unwrap();
        db.upsert_project(&ProjectRow {
            id: "proj".into(),
            path: "/proj".into(),
            name: "proj".into(),
            git_remote: None,
            discovered_at: 1000,
            last_seen_at: 1000,
            last_scanned_at: 0,
            tools: vec![],
        })
        .unwrap();

        db.upsert_session(&SessionRow {
            id: "s1".into(),
            source: "cursor".into(),
            project_id: "proj".into(),
            name: Some("test".into()),
            mode: None,
            model: None,
            created_at: Some(5000),
            updated_at: Some(6000),
            message_count: 3,
            first_message: None,
            indexed_at: 7000,
        })
        .unwrap();

        let windows = build_session_windows(&db).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].project_id, "proj");
        assert_eq!(windows[0].source, "cursor");
        assert_eq!(windows[0].start_epoch, 5000);
        assert_eq!(windows[0].end_epoch, 6000 + SESSION_BUFFER_SECS);
    }

    #[test]
    fn test_build_session_windows_skips_null_created_at() {
        use crate::db::projects::ProjectRow;
        use crate::db::sessions::SessionRow;
        use crate::db::Db;

        let db = Db::open_in_memory().unwrap();
        db.upsert_project(&ProjectRow {
            id: "proj".into(),
            path: "/proj".into(),
            name: "proj".into(),
            git_remote: None,
            discovered_at: 1000,
            last_seen_at: 1000,
            last_scanned_at: 0,
            tools: vec![],
        })
        .unwrap();

        db.upsert_session(&SessionRow {
            id: "s-no-ts".into(),
            source: "cursor".into(),
            project_id: "proj".into(),
            name: None,
            mode: None,
            model: None,
            created_at: None,
            updated_at: None,
            message_count: 0,
            first_message: None,
            indexed_at: 0,
        })
        .unwrap();

        let windows = build_session_windows(&db).unwrap();
        assert!(windows.is_empty());
    }

    #[test]
    fn test_build_session_windows_normalizes_millis() {
        use crate::db::projects::ProjectRow;
        use crate::db::sessions::SessionRow;
        use crate::db::Db;

        let db = Db::open_in_memory().unwrap();
        db.upsert_project(&ProjectRow {
            id: "proj".into(),
            path: "/proj".into(),
            name: "proj".into(),
            git_remote: None,
            discovered_at: 1000,
            last_seen_at: 1000,
            last_scanned_at: 0,
            tools: vec![],
        })
        .unwrap();

        db.upsert_session(&SessionRow {
            id: "s-millis".into(),
            source: "cursor".into(),
            project_id: "proj".into(),
            name: None,
            mode: None,
            model: None,
            created_at: Some(1_700_000_000_000),
            updated_at: Some(1_700_001_000_000),
            message_count: 1,
            first_message: None,
            indexed_at: 0,
        })
        .unwrap();

        let windows = build_session_windows(&db).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].start_epoch, 1_700_000_000);
        assert_eq!(windows[0].end_epoch, 1_700_001_000 + SESSION_BUFFER_SECS);
    }
}
