#![allow(dead_code)]

use std::collections::HashMap;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::Db;

#[derive(Debug, Clone, Default)]
pub struct DailyActivity {
    pub project_id: String,
    pub date: String,
    pub commits: i64,
    pub lines_added: i64,
    pub lines_deleted: i64,
    pub files_changed: i64,
    pub authors: Vec<String>,
    pub ai_assisted_commits: i64,
}

pub fn ingest_git_activity(db: &Db, force: bool) -> Result<(usize, usize), String> {
    let projects = db.list_projects()?;
    let mut total_ingested = 0usize;
    let mut total_skipped = 0usize;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    for project in &projects {
        if project.path.is_empty() || !std::path::Path::new(&project.path).join(".git").exists() {
            continue;
        }

        let mut daily = parse_daily_activity(&project.path, &project.id, 90)?;
        populate_ai_assisted(db, &project.path, &mut daily);

        for day in &daily {
            if !force {
                let exists: i64 = db
                    .conn
                    .query_row(
                        "SELECT COUNT(*) FROM git_activity WHERE project_id = ?1 AND date = ?2",
                        rusqlite::params![project.id, day.date],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                if exists > 0 {
                    total_skipped += 1;
                    continue;
                }
            }

            let authors_json =
                serde_json::to_string(&day.authors).unwrap_or_else(|_| "[]".to_string());

            db.conn
                .execute(
                    "INSERT INTO git_activity (project_id, date, commits, lines_added, lines_deleted, files_changed, authors, ai_assisted_commits, ingested_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT(project_id, date) DO UPDATE SET
                         commits = excluded.commits,
                         lines_added = excluded.lines_added,
                         lines_deleted = excluded.lines_deleted,
                         files_changed = excluded.files_changed,
                         authors = excluded.authors,
                         ai_assisted_commits = excluded.ai_assisted_commits,
                         ingested_at = excluded.ingested_at",
                    rusqlite::params![
                        day.project_id,
                        day.date,
                        day.commits,
                        day.lines_added,
                        day.lines_deleted,
                        day.files_changed,
                        authors_json,
                        day.ai_assisted_commits,
                        now,
                    ],
                )
                .map_err(|e| format!("cannot upsert git_activity: {e}"))?;

            total_ingested += 1;
        }
    }

    Ok((total_ingested, total_skipped))
}

/// Cross-reference anchors to count AI-assisted commits per day.
/// Only counts commits that actually belong to this project by getting
/// commit hashes from the project's git log first.
fn populate_ai_assisted(db: &Db, project_path: &str, daily: &mut [DailyActivity]) {
    if daily.is_empty() {
        return;
    }

    let days = daily.len() as u32;
    let since = format!("--since={days} days ago");
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let output = Command::new(git)
        .args(["log", &since, "--format=%H"])
        .current_dir(project_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();

    let hashes: std::collections::HashSet<String> = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|h| !h.is_empty())
            .collect(),
        _ => return,
    };

    if hashes.is_empty() {
        return;
    }

    let ai_dates: HashMap<String, i64> = match db
        .conn
        .prepare("SELECT commit_hash, raw_json FROM anchors WHERE raw_json IS NOT NULL")
    {
        Ok(mut stmt) => {
            let mut map = HashMap::new();
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                for row in rows.flatten() {
                    let (hash, json) = row;
                    if !hashes.contains(&hash) {
                        continue;
                    }
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                        let atype = v["author_type"].as_str().unwrap_or("human");
                        if atype == "agent" || atype == "assisted" {
                            let ts = v["committed_at"].as_i64().unwrap_or(0);
                            let date = epoch_to_date(ts);
                            *map.entry(date).or_insert(0) += 1;
                        }
                    }
                }
            }
            map
        }
        Err(_) => HashMap::new(),
    };

    for day in daily.iter_mut() {
        if let Some(&count) = ai_dates.get(&day.date) {
            day.ai_assisted_commits = count;
        }
    }
}

fn parse_daily_activity(
    project_path: &str,
    project_id: &str,
    days: u32,
) -> Result<Vec<DailyActivity>, String> {
    let since = format!("--since={days} days ago");
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let output = Command::new(git)
        .args([
            "-C",
            project_path,
            "log",
            "--all",
            &since,
            "--format=%at|%ae",
            "--shortstat",
        ])
        .output()
        .map_err(|e| format!("git log failed in {project_path}: {e}"))?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut daily_map: HashMap<String, DailyActivity> = HashMap::new();

    let mut current_date: Option<String> = None;
    let mut current_author: Option<String> = None;

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some((epoch_str, author)) = line.split_once('|') {
            if let Ok(epoch) = epoch_str.parse::<i64>() {
                let date = epoch_to_date(epoch);
                current_author = Some(author.to_string());

                let entry = daily_map.entry(date).or_insert_with_key(|d| DailyActivity {
                    project_id: project_id.to_string(),
                    date: d.clone(),
                    ..Default::default()
                });
                current_date = Some(entry.date.clone());
                entry.commits += 1;
                if !entry.authors.contains(&author.to_string()) {
                    entry.authors.push(author.to_string());
                }
            }
        } else if let Some(ref date) = current_date {
            let (files, added, deleted) = parse_shortstat(line);
            if let Some(entry) = daily_map.get_mut(date) {
                entry.files_changed += files;
                entry.lines_added += added;
                entry.lines_deleted += deleted;
            }
            current_date = None;
            current_author = None;
        }
        let _ = &current_author;
    }

    let mut result: Vec<DailyActivity> = daily_map.into_values().collect();
    result.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(result)
}

fn parse_shortstat(line: &str) -> (i64, i64, i64) {
    let mut files = 0i64;
    let mut insertions = 0i64;
    let mut deletions = 0i64;

    for part in line.split(',') {
        let part = part.trim();
        if part.contains("file") {
            files = extract_number(part);
        } else if part.contains("insertion") {
            insertions = extract_number(part);
        } else if part.contains("deletion") {
            deletions = extract_number(part);
        }
    }

    (files, insertions, deletions)
}

fn extract_number(s: &str) -> i64 {
    s.split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

fn epoch_to_date(epoch: i64) -> String {
    chrono::DateTime::from_timestamp(epoch, 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

impl Db {
    pub fn git_activity_global(&self, days: usize) -> Result<Vec<GitActivityRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT date, SUM(commits), SUM(lines_added), SUM(lines_deleted),
                    SUM(files_changed), SUM(ai_assisted_commits)
             FROM git_activity
             GROUP BY date
             ORDER BY date DESC
             LIMIT ?1",
            )
            .map_err(|e| format!("cannot query git_activity: {e}"))?;

        let rows = stmt
            .query_map(rusqlite::params![days as i64], |row| {
                Ok(GitActivityRow {
                    date: row.get(0)?,
                    commits: row.get(1)?,
                    lines_added: row.get(2)?,
                    lines_deleted: row.get(3)?,
                    files_changed: row.get(4)?,
                    ai_assisted_commits: row.get(5)?,
                })
            })
            .map_err(|e| format!("git_activity query error: {e}"))?;

        crate::db::collect_rows(rows)
    }

    pub fn git_activity_by_project(
        &self,
        project_id: &str,
        days: usize,
    ) -> Result<Vec<GitActivityRow>, String> {
        let mut stmt = self.conn.prepare(
            "SELECT date, commits, lines_added, lines_deleted, files_changed, ai_assisted_commits
             FROM git_activity
             WHERE project_id = ?1
             ORDER BY date DESC
             LIMIT ?2",
        ).map_err(|e| format!("cannot query git_activity: {e}"))?;

        let rows = stmt
            .query_map(rusqlite::params![project_id, days as i64], |row| {
                Ok(GitActivityRow {
                    date: row.get(0)?,
                    commits: row.get(1)?,
                    lines_added: row.get(2)?,
                    lines_deleted: row.get(3)?,
                    files_changed: row.get(4)?,
                    ai_assisted_commits: row.get(5)?,
                })
            })
            .map_err(|e| format!("git_activity query error: {e}"))?;

        crate::db::collect_rows(rows)
    }

    pub fn productivity_summary(&self) -> Result<ProductivitySummary, String> {
        let row = self
            .conn
            .query_row(
                "SELECT COUNT(DISTINCT date), SUM(commits), SUM(lines_added), SUM(lines_deleted),
                    SUM(files_changed), SUM(ai_assisted_commits)
             FROM git_activity",
                [],
                |row| {
                    Ok(ProductivitySummary {
                        active_days: row.get(0)?,
                        total_commits: row.get(1)?,
                        total_lines_added: row.get(2)?,
                        total_lines_deleted: row.get(3)?,
                        total_files_changed: row.get(4)?,
                        total_ai_assisted: row.get(5)?,
                    })
                },
            )
            .map_err(|e| format!("cannot compute productivity summary: {e}"))?;

        let session_days: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(DISTINCT d) FROM (
                    SELECT date(CASE WHEN COALESCE(updated_at, created_at) >= 1000000000000000 THEN COALESCE(updated_at, created_at) / 1000000
                                     WHEN COALESCE(updated_at, created_at) >= 1000000000000 THEN COALESCE(updated_at, created_at) / 1000
                                     ELSE COALESCE(updated_at, created_at) END,
                                'unixepoch', 'localtime') AS d
                    FROM sessions
                    WHERE COALESCE(updated_at, created_at) IS NOT NULL
                      AND COALESCE(updated_at, created_at) > 0
                    UNION
                    SELECT date FROM git_activity
                )",
                [],
                |r| r.get(0),
            )
            .unwrap_or(row.active_days);

        Ok(ProductivitySummary {
            active_days: session_days,
            ..row
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct GitActivityRow {
    pub date: String,
    pub commits: i64,
    pub lines_added: i64,
    pub lines_deleted: i64,
    pub files_changed: i64,
    pub ai_assisted_commits: i64,
}

#[derive(Debug, Clone, Default)]
pub struct ProductivitySummary {
    pub active_days: i64,
    pub total_commits: i64,
    pub total_lines_added: i64,
    pub total_lines_deleted: i64,
    pub total_files_changed: i64,
    pub total_ai_assisted: i64,
}

impl ProductivitySummary {
    pub fn commits_per_day(&self) -> f64 {
        if self.active_days > 0 {
            self.total_commits as f64 / self.active_days as f64
        } else {
            0.0
        }
    }

    pub fn lines_per_day(&self) -> f64 {
        if self.active_days > 0 {
            self.total_lines_added as f64 / self.active_days as f64
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_shortstat() {
        let (f, a, d) = parse_shortstat(" 3 files changed, 42 insertions(+), 10 deletions(-)");
        assert_eq!(f, 3);
        assert_eq!(a, 42);
        assert_eq!(d, 10);
    }

    #[test]
    fn test_parse_shortstat_partial() {
        let (f, a, d) = parse_shortstat(" 1 file changed, 5 insertions(+)");
        assert_eq!(f, 1);
        assert_eq!(a, 5);
        assert_eq!(d, 0);
    }

    #[test]
    fn test_productivity_summary_calculations() {
        let p = ProductivitySummary {
            active_days: 10,
            total_commits: 50,
            total_lines_added: 2000,
            ..Default::default()
        };
        assert!((p.commits_per_day() - 5.0).abs() < 0.01);
        assert!((p.lines_per_day() - 200.0).abs() < 0.01);
    }
}
