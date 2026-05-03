#![allow(dead_code)]

use rusqlite::params;

use super::Db;

#[derive(Debug, Clone)]
pub struct AiCommitRow {
    pub commit_hash: String,
    pub branch_name: String,
    pub project_id: Option<String>,
    pub commit_message: Option<String>,
    pub commit_date: Option<String>,
    pub lines_added: i64,
    pub lines_deleted: i64,
    pub ai_lines_added: i64,
    pub ai_lines_deleted: i64,
    pub tab_lines_added: i64,
    pub tab_lines_deleted: i64,
    pub human_lines_added: i64,
    pub human_lines_deleted: i64,
    pub ai_percentage: Option<f64>,
    pub source: String,
    pub ingested_at: i64,
}

#[derive(Debug, Clone, Default)]
pub struct AiCommitSummary {
    pub total_commits: i64,
    pub total_lines_added: i64,
    pub total_lines_deleted: i64,
    pub ai_lines_added: i64,
    pub ai_lines_deleted: i64,
    pub human_lines_added: i64,
    pub human_lines_deleted: i64,
    pub tab_lines_added: i64,
    pub avg_ai_percentage: f64,
}

impl Db {
    pub fn upsert_ai_commit(&self, row: &AiCommitRow) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO ai_commits (commit_hash, branch_name, project_id, commit_message, commit_date,
                    lines_added, lines_deleted, ai_lines_added, ai_lines_deleted,
                    tab_lines_added, tab_lines_deleted, human_lines_added, human_lines_deleted,
                    ai_percentage, source, ingested_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                 ON CONFLICT(commit_hash, branch_name) DO UPDATE SET
                    project_id = COALESCE(excluded.project_id, ai_commits.project_id),
                    commit_message = COALESCE(excluded.commit_message, ai_commits.commit_message),
                    commit_date = COALESCE(excluded.commit_date, ai_commits.commit_date),
                    lines_added = excluded.lines_added,
                    lines_deleted = excluded.lines_deleted,
                    ai_lines_added = excluded.ai_lines_added,
                    ai_lines_deleted = excluded.ai_lines_deleted,
                    tab_lines_added = excluded.tab_lines_added,
                    tab_lines_deleted = excluded.tab_lines_deleted,
                    human_lines_added = excluded.human_lines_added,
                    human_lines_deleted = excluded.human_lines_deleted,
                    ai_percentage = excluded.ai_percentage,
                    source = excluded.source,
                    ingested_at = excluded.ingested_at",
                params![
                    row.commit_hash,
                    row.branch_name,
                    row.project_id,
                    row.commit_message,
                    row.commit_date,
                    row.lines_added,
                    row.lines_deleted,
                    row.ai_lines_added,
                    row.ai_lines_deleted,
                    row.tab_lines_added,
                    row.tab_lines_deleted,
                    row.human_lines_added,
                    row.human_lines_deleted,
                    row.ai_percentage,
                    row.source,
                    row.ingested_at,
                ],
            )
            .map_err(|e| format!("cannot upsert ai_commit: {e}"))?;
        Ok(())
    }

    pub fn ai_commit_count(&self) -> Result<i64, String> {
        self.conn
            .query_row(
                "SELECT COUNT(DISTINCT commit_hash) FROM ai_commits",
                [],
                |r| r.get(0),
            )
            .map_err(|e| format!("cannot count ai_commits: {e}"))
    }

    pub fn ai_commit_summary_global(&self) -> Result<AiCommitSummary, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*),
                    COALESCE(SUM(lines_added), 0),
                    COALESCE(SUM(lines_deleted), 0),
                    COALESCE(SUM(ai_lines_added), 0),
                    COALESCE(SUM(ai_lines_deleted), 0),
                    COALESCE(SUM(human_lines_added), 0),
                    COALESCE(SUM(human_lines_deleted), 0),
                    COALESCE(SUM(tab_lines_added), 0),
                    COALESCE(AVG(ai_percentage), 0.0)
                 FROM ai_commits",
                [],
                |row| {
                    Ok(AiCommitSummary {
                        total_commits: row.get(0)?,
                        total_lines_added: row.get(1)?,
                        total_lines_deleted: row.get(2)?,
                        ai_lines_added: row.get(3)?,
                        ai_lines_deleted: row.get(4)?,
                        human_lines_added: row.get(5)?,
                        human_lines_deleted: row.get(6)?,
                        tab_lines_added: row.get(7)?,
                        avg_ai_percentage: row.get(8)?,
                    })
                },
            )
            .map_err(|e| format!("cannot aggregate ai_commits: {e}"))
    }

    pub fn ai_commit_summary_by_project(
        &self,
        project_id: &str,
    ) -> Result<AiCommitSummary, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*),
                    COALESCE(SUM(lines_added), 0),
                    COALESCE(SUM(lines_deleted), 0),
                    COALESCE(SUM(ai_lines_added), 0),
                    COALESCE(SUM(ai_lines_deleted), 0),
                    COALESCE(SUM(human_lines_added), 0),
                    COALESCE(SUM(human_lines_deleted), 0),
                    COALESCE(SUM(tab_lines_added), 0),
                    COALESCE(AVG(ai_percentage), 0.0)
                 FROM ai_commits WHERE project_id = ?1",
                params![project_id],
                |row| {
                    Ok(AiCommitSummary {
                        total_commits: row.get(0)?,
                        total_lines_added: row.get(1)?,
                        total_lines_deleted: row.get(2)?,
                        ai_lines_added: row.get(3)?,
                        ai_lines_deleted: row.get(4)?,
                        human_lines_added: row.get(5)?,
                        human_lines_deleted: row.get(6)?,
                        tab_lines_added: row.get(7)?,
                        avg_ai_percentage: row.get(8)?,
                    })
                },
            )
            .map_err(|e| format!("cannot aggregate ai_commits by project: {e}"))
    }

    /// Weekly AI code attribution trend: AI% per week for the last N weeks.
    pub fn weekly_ai_trend(&self, weeks: usize) -> Result<Vec<AiWeeklyTrend>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT week, COUNT(*), COALESCE(SUM(lines_added), 0),
                    COALESCE(SUM(ai_lines_added), 0), COALESCE(SUM(human_lines_added), 0),
                    COALESCE(AVG(ai_pct), 0.0)
                 FROM (
                    SELECT commit_hash,
                        strftime('%Y-W%W', COALESCE(
                            datetime(commit_epoch, 'unixepoch'),
                            datetime(commit_date)
                        )) AS week,
                        MAX(lines_added) AS lines_added,
                        MAX(ai_lines_added) AS ai_lines_added,
                        MAX(human_lines_added) AS human_lines_added,
                        MAX(ai_percentage) AS ai_pct
                    FROM ai_commits
                    WHERE commit_epoch IS NOT NULL OR commit_date IS NOT NULL
                    GROUP BY commit_hash
                    HAVING week IS NOT NULL
                 )
                 GROUP BY week
                 ORDER BY week DESC
                 LIMIT ?1",
            )
            .map_err(|e| format!("cannot prepare weekly trend: {e}"))?;

        let rows = stmt
            .query_map(rusqlite::params![weeks as i64], |row| {
                Ok(AiWeeklyTrend {
                    week: row.get(0)?,
                    commits: row.get(1)?,
                    lines_added: row.get(2)?,
                    ai_lines: row.get(3)?,
                    human_lines: row.get(4)?,
                    avg_ai_pct: row.get(5)?,
                })
            })
            .map_err(|e| format!("weekly trend query error: {e}"))?;

        super::collect_rows(rows)
    }

    /// Monthly AI code attribution summary.
    pub fn monthly_ai_trend(&self, months: usize) -> Result<Vec<AiMonthlyTrend>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT month, COUNT(*), COALESCE(SUM(lines_added), 0),
                    COALESCE(SUM(ai_lines_added), 0), COALESCE(SUM(human_lines_added), 0),
                    COALESCE(AVG(ai_pct), 0.0)
                 FROM (
                    SELECT commit_hash,
                        strftime('%Y-%m', COALESCE(
                            datetime(commit_epoch, 'unixepoch'),
                            datetime(commit_date)
                        )) AS month,
                        MAX(lines_added) AS lines_added,
                        MAX(ai_lines_added) AS ai_lines_added,
                        MAX(human_lines_added) AS human_lines_added,
                        MAX(ai_percentage) AS ai_pct
                    FROM ai_commits
                    WHERE commit_epoch IS NOT NULL OR commit_date IS NOT NULL
                    GROUP BY commit_hash
                    HAVING month IS NOT NULL
                 )
                 GROUP BY month
                 ORDER BY month DESC
                 LIMIT ?1",
            )
            .map_err(|e| format!("cannot prepare monthly trend: {e}"))?;

        let rows = stmt
            .query_map(rusqlite::params![months as i64], |row| {
                Ok(AiMonthlyTrend {
                    month: row.get(0)?,
                    commits: row.get(1)?,
                    lines_added: row.get(2)?,
                    ai_lines: row.get(3)?,
                    human_lines: row.get(4)?,
                    avg_ai_pct: row.get(5)?,
                })
            })
            .map_err(|e| format!("monthly trend query error: {e}"))?;

        super::collect_rows(rows)
    }

    /// Headline metric: AI code percentage for a given time range.
    pub fn ai_code_percentage(
        &self,
        since_epoch: Option<i64>,
        until_epoch: Option<i64>,
    ) -> Result<AiCodeHeadline, String> {
        let (query, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) =
            match (since_epoch, until_epoch) {
                (Some(s), Some(u)) => (
                    "SELECT COUNT(*), COALESCE(SUM(lines_added), 0), COALESCE(SUM(ai_lines_added), 0), COALESCE(SUM(human_lines_added), 0)
                     FROM (SELECT commit_hash, MAX(lines_added) AS lines_added, MAX(ai_lines_added) AS ai_lines_added, MAX(human_lines_added) AS human_lines_added
                           FROM ai_commits WHERE source != 'correlation:human' AND commit_epoch >= ?1 AND commit_epoch <= ?2 GROUP BY commit_hash)",
                    vec![Box::new(s), Box::new(u)],
                ),
                (Some(s), None) => (
                    "SELECT COUNT(*), COALESCE(SUM(lines_added), 0), COALESCE(SUM(ai_lines_added), 0), COALESCE(SUM(human_lines_added), 0)
                     FROM (SELECT commit_hash, MAX(lines_added) AS lines_added, MAX(ai_lines_added) AS ai_lines_added, MAX(human_lines_added) AS human_lines_added
                           FROM ai_commits WHERE source != 'correlation:human' AND commit_epoch >= ?1 GROUP BY commit_hash)",
                    vec![Box::new(s)],
                ),
                _ => (
                    "SELECT COUNT(*), COALESCE(SUM(lines_added), 0), COALESCE(SUM(ai_lines_added), 0), COALESCE(SUM(human_lines_added), 0)
                     FROM (SELECT commit_hash, MAX(lines_added) AS lines_added, MAX(ai_lines_added) AS ai_lines_added, MAX(human_lines_added) AS human_lines_added
                           FROM ai_commits WHERE source != 'correlation:human' GROUP BY commit_hash)",
                    vec![],
                ),
            };

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        self.conn
            .query_row(query, param_refs.as_slice(), |row| {
                let total_commits: i64 = row.get(0)?;
                let total_lines: i64 = row.get(1)?;
                let ai_lines: i64 = row.get(2)?;
                let human_lines: i64 = row.get(3)?;
                let ai_pct = if total_lines > 0 {
                    100.0 * ai_lines as f64 / total_lines as f64
                } else {
                    0.0
                };
                Ok(AiCodeHeadline {
                    total_commits,
                    total_lines,
                    ai_lines,
                    human_lines,
                    ai_percentage: ai_pct,
                })
            })
            .map_err(|e| format!("cannot compute AI code headline: {e}"))
    }

    pub fn ai_commit_summary_top_branches(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, AiCommitSummary)>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT branch_name, COUNT(*),
                    COALESCE(SUM(lines_added), 0),
                    COALESCE(SUM(lines_deleted), 0),
                    COALESCE(SUM(ai_lines_added), 0),
                    COALESCE(SUM(ai_lines_deleted), 0),
                    COALESCE(SUM(human_lines_added), 0),
                    COALESCE(SUM(human_lines_deleted), 0),
                    COALESCE(SUM(tab_lines_added), 0),
                    COALESCE(AVG(ai_percentage), 0.0)
             FROM ai_commits
             GROUP BY branch_name
             ORDER BY SUM(lines_added) DESC
             LIMIT ?1",
            )
            .map_err(|e| format!("cannot prepare branch query: {e}"))?;

        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    AiCommitSummary {
                        total_commits: row.get(1)?,
                        total_lines_added: row.get(2)?,
                        total_lines_deleted: row.get(3)?,
                        ai_lines_added: row.get(4)?,
                        ai_lines_deleted: row.get(5)?,
                        human_lines_added: row.get(6)?,
                        human_lines_deleted: row.get(7)?,
                        tab_lines_added: row.get(8)?,
                        avg_ai_percentage: row.get(9)?,
                    },
                ))
            })
            .map_err(|e| format!("branch query error: {e}"))?;

        super::collect_rows(rows)
    }
}

#[derive(Debug, Clone, Default)]
pub struct AiWeeklyTrend {
    pub week: String,
    pub commits: i64,
    pub lines_added: i64,
    pub ai_lines: i64,
    pub human_lines: i64,
    pub avg_ai_pct: f64,
}

impl AiWeeklyTrend {
    pub fn ai_percentage(&self) -> f64 {
        if self.lines_added > 0 {
            100.0 * self.ai_lines as f64 / self.lines_added as f64
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AiMonthlyTrend {
    pub month: String,
    pub commits: i64,
    pub lines_added: i64,
    pub ai_lines: i64,
    pub human_lines: i64,
    pub avg_ai_pct: f64,
}

impl AiMonthlyTrend {
    pub fn ai_percentage(&self) -> f64 {
        if self.lines_added > 0 {
            100.0 * self.ai_lines as f64 / self.lines_added as f64
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AiCodeHeadline {
    pub total_commits: i64,
    pub total_lines: i64,
    pub ai_lines: i64,
    pub human_lines: i64,
    pub ai_percentage: f64,
}
