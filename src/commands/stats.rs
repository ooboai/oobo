use std::io::IsTerminal;

use crate::db::Db;

pub fn run(
    project: Option<String>,
    tool: Option<String>,
    agent: bool,
    since: Option<String>,
) -> Result<(), String> {
    let db = Db::open()?;

    let since_ts = since.as_deref().and_then(parse_since);

    let (stats, scope_label) = if let Some(ref proj) = project {
        let p = find_project_id(&db, proj)?;
        let name = db
            .get_project_by_id(&p)?
            .map(|pr| pr.name)
            .unwrap_or_else(|| proj.clone());
        (db.aggregate_stats_by_project(&p)?, name)
    } else if let Some(ref t) = tool {
        (
            db.aggregate_stats_by_tool(t)?,
            crate::tui::source_label(t).to_string(),
        )
    } else {
        (
            db.aggregate_stats_global_since(since_ts)?,
            "all projects".to_string(),
        )
    };

    if agent {
        print_json(&stats, &project, &tool);
        return Ok(());
    }

    if stats.session_count == 0 {
        eprintln!("no indexed data — run `oobo scan` then `oobo index`");
        return Ok(());
    }

    if std::io::stdin().is_terminal() {
        let is_global = project.is_none() && tool.is_none();

        let per_tool = if is_global {
            db.aggregate_stats_per_tool()?
        } else {
            Vec::new()
        };

        let top_projects = if is_global {
            db.aggregate_stats_top_projects(10)?
        } else {
            Vec::new()
        };

        let daily = db.daily_stats(14)?;
        let data_sources = build_data_sources(&per_tool);

        let cursor_code_stats = if is_global {
            crate::tools::cursor::composer_data::read_daily_code_stats()
        } else {
            Vec::new()
        };

        let ai_commits = if is_global {
            db.ai_commit_summary_global().ok()
        } else if let Some(ref proj) = project {
            let pid = find_project_id(&db, proj).ok();
            pid.and_then(|p| db.ai_commit_summary_by_project(&p).ok())
        } else {
            None
        };

        let ai_branches = if is_global {
            db.ai_commit_summary_top_branches(8).unwrap_or_default()
        } else {
            Vec::new()
        };

        let ai_headline = if is_global {
            db.ai_code_percentage(None, None).ok()
        } else {
            None
        };

        let ai_weekly_trend = if is_global {
            db.weekly_ai_trend(8).unwrap_or_default()
        } else {
            Vec::new()
        };

        let per_model = if is_global {
            db.aggregate_stats_by_model().unwrap_or_default()
        } else {
            Vec::new()
        };

        let productivity = if is_global {
            db.productivity_summary().ok()
        } else {
            None
        };

        let git_activity = if is_global {
            db.git_activity_global(14).unwrap_or_default()
        } else {
            Vec::new()
        };

        let view = crate::tui::stats::StatsView {
            scope_label,
            global: stats,
            per_tool,
            top_projects,
            daily,
            data_sources,
            cursor_code_stats,
            ai_commits,
            ai_branches,
            ai_headline,
            ai_weekly_trend,
            per_model,
            productivity,
            git_activity,
        };

        crate::tui::stats::run(view)?;
    } else {
        print_plain(
            &stats,
            &scope_label,
            &db,
            project.is_none() && tool.is_none(),
        )?;
    }

    Ok(())
}

fn find_project_id(db: &Db, name: &str) -> Result<String, String> {
    crate::commands::index::find_project_id(db, name)
}

fn print_plain(
    stats: &crate::db::stats::AggregateStats,
    scope: &str,
    db: &Db,
    show_breakdown: bool,
) -> Result<(), String> {
    let total = stats.total_input_tokens + stats.total_output_tokens;
    println!("oobo stats — {scope}");
    println!("Sessions:       {}", stats.session_count);
    println!(
        "Input tokens:   {}",
        crate::tui::format_tokens(stats.total_input_tokens)
    );
    println!(
        "Output tokens:  {}",
        crate::tui::format_tokens(stats.total_output_tokens)
    );
    println!("Total tokens:   {}", crate::tui::format_tokens(total));
    if stats.total_duration_secs > 0 {
        println!(
            "Total time:     {}",
            crate::tui::format_duration(stats.total_duration_secs)
        );
    }

    if show_breakdown {
        let per_tool = db.aggregate_stats_per_tool()?;
        if !per_tool.is_empty() {
            println!();
            for (source, s) in &per_tool {
                let label = crate::tui::source_label(source);
                println!(
                    "  {:<12} {:>6} sessions  {:>8} in  {:>8} out",
                    label,
                    s.session_count,
                    crate::tui::format_tokens(s.total_input_tokens),
                    crate::tui::format_tokens(s.total_output_tokens),
                );
            }
        }

        if let Ok(api_totals) = db.api_usage_totals() {
            if !api_totals.is_empty() {
                println!();
                println!("Remote API usage (last 30 days):");
                for (source, summary) in &api_totals {
                    if summary.has_data() {
                        let label = match source.as_str() {
                            "anthropic" => "Anthropic",
                            "openai" => "OpenAI",
                            "cursor" => "Cursor API",
                            _ => source.as_str(),
                        };
                        let tokens = summary.total_tokens() as i64;
                        println!(
                            "  {:<12} {:>8} tokens",
                            label,
                            crate::tui::format_tokens(tokens),
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

fn build_data_sources(
    per_tool: &[(String, crate::db::stats::AggregateStats)],
) -> Vec<(String, String)> {
    let mut sources = Vec::new();
    for (source, _) in per_tool {
        let label = crate::tui::source_label(source).to_string();
        let desc = match source.as_str() {
            "claude" => "native tokens + cost",
            "codex" | "opencode" => "native tokens + cost",
            "gemini" => "native tokens, est. cost",
            "aider" => "native telemetry",
            "zed" => "native telemetry",
            "composer" | "cursor" => "estimated (tiktoken)",
            "copilot" => "discovery only",
            "windsurf" | "trae" => "discovery only",
            _ => "estimated (tiktoken)",
        };
        sources.push((label, desc.to_string()));
    }
    sources
}

fn print_json(
    stats: &crate::db::stats::AggregateStats,
    project: &Option<String>,
    tool: &Option<String>,
) {
    let db = match Db::open() {
        Ok(d) => d,
        Err(_) => {
            let json = serde_json::json!({
                "sessions": stats.session_count,
                "input_tokens": stats.total_input_tokens,
                "output_tokens": stats.total_output_tokens,
            });
            crate::utils::print_json(&json);
            return;
        }
    };

    let mut api_usage = serde_json::Map::new();
    if let Ok(totals) = db.api_usage_totals() {
        for (source, summary) in totals {
            if summary.has_data() {
                api_usage.insert(
                    source,
                    serde_json::json!({
                        "input_tokens": summary.input_tokens,
                        "output_tokens": summary.output_tokens,
                        "cache_read_tokens": summary.cache_read_tokens,
                        "days": summary.days,
                    }),
                );
            }
        }
    }

    let per_tool: Vec<serde_json::Value> = db
        .aggregate_stats_per_tool()
        .unwrap_or_default()
        .iter()
        .map(|(source, s)| {
            serde_json::json!({
                "tool": source,
                "sessions": s.session_count,
                "input_tokens": s.total_input_tokens,
                "output_tokens": s.total_output_tokens,
            })
        })
        .collect();

    let per_model: Vec<serde_json::Value> = db
        .aggregate_stats_by_model()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            serde_json::json!({
                "model": m.model,
                "sessions": m.session_count,
                "input_tokens": m.input_tokens,
                "output_tokens": m.output_tokens,
                "pct_of_total": m.pct_of_total_output,
            })
        })
        .collect();

    let ai_code = db.ai_code_percentage(None, None).ok().map(|a| {
        serde_json::json!({
            "total_commits": a.total_commits,
            "total_lines": a.total_lines,
            "ai_lines": a.ai_lines,
            "human_lines": a.human_lines,
            "ai_percentage": a.ai_percentage,
        })
    });

    let productivity = db.productivity_summary().ok().map(|p| {
        serde_json::json!({
            "active_days": p.active_days,
            "total_commits": p.total_commits,
            "lines_added": p.total_lines_added,
            "lines_deleted": p.total_lines_deleted,
            "commits_per_day": p.commits_per_day(),
            "lines_per_day": p.lines_per_day(),
        })
    });

    let daily: Vec<serde_json::Value> = db
        .daily_stats(30)
        .unwrap_or_default()
        .iter()
        .map(|(date, s)| {
            serde_json::json!({
                "date": date,
                "sessions": s.session_count,
                "input_tokens": s.total_input_tokens,
                "output_tokens": s.total_output_tokens,
            })
        })
        .collect();

    let json = serde_json::json!({
        "scope": {
            "project": project,
            "tool": tool,
        },
        "sessions": stats.session_count,
        "input_tokens": stats.total_input_tokens,
        "output_tokens": stats.total_output_tokens,
        "total_tokens": stats.total_input_tokens + stats.total_output_tokens,
        "total_duration_secs": stats.total_duration_secs,
        "api_usage": api_usage,
        "per_tool": per_tool,
        "per_model": per_model,
        "ai_code": ai_code,
        "productivity": productivity,
        "daily": daily,
    });
    crate::utils::print_json(&json);
}

/// Parse relative time strings like "7d", "30d", "2w", "3m" into a Unix timestamp.
fn parse_since(s: &str) -> Option<i64> {
    let s = s.trim();
    let (num_str, unit) = if let Some(stripped) = s.strip_suffix('d') {
        (stripped, 86400i64)
    } else if let Some(stripped) = s.strip_suffix('w') {
        (stripped, 7 * 86400)
    } else if let Some(stripped) = s.strip_suffix('m') {
        (stripped, 30 * 86400)
    } else {
        return None;
    };
    let n: i64 = num_str.parse().ok()?;
    Some(chrono::Utc::now().timestamp() - n * unit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::projects::ProjectRow;
    use crate::db::sessions::SessionRow;
    use crate::db::stats::StatsRow;

    #[test]
    fn test_parse_since_days() {
        let now = chrono::Utc::now().timestamp();
        let ts = parse_since("7d").unwrap();
        let diff = now - ts;
        assert!((diff - 7 * 86400).abs() < 2, "7d should be ~7 days ago");

        let ts = parse_since("30d").unwrap();
        let diff = now - ts;
        assert!((diff - 30 * 86400).abs() < 2, "30d should be ~30 days ago");
    }

    #[test]
    fn test_parse_since_weeks() {
        let now = chrono::Utc::now().timestamp();
        let ts = parse_since("2w").unwrap();
        let diff = now - ts;
        assert!((diff - 14 * 86400).abs() < 2, "2w should be ~14 days ago");
    }

    #[test]
    fn test_parse_since_months() {
        let now = chrono::Utc::now().timestamp();
        let ts = parse_since("3m").unwrap();
        let diff = now - ts;
        assert!((diff - 90 * 86400).abs() < 2, "3m should be ~90 days ago");
    }

    #[test]
    fn test_parse_since_invalid() {
        assert!(parse_since("abc").is_none());
        assert!(parse_since("").is_none());
    }

    #[test]
    fn test_parse_since_1d() {
        let now = chrono::Utc::now().timestamp();
        let ts = parse_since("1d").unwrap();
        let diff = now - ts;
        assert!((diff - 86400).abs() < 2);
    }

    #[test]
    fn test_parse_since_no_number() {
        assert!(parse_since("d").is_none());
        assert!(parse_since("w").is_none());
        assert!(parse_since("m").is_none());
    }

    fn seed_db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.upsert_project(&ProjectRow {
            id: "proj-1".into(),
            path: "/tmp/test-proj".into(),
            name: "test-proj".into(),
            git_remote: None,
            discovered_at: 1000,
            last_seen_at: 1000,
            last_scanned_at: 0,
            tools: vec![],
        })
        .unwrap();
        db.upsert_session(&SessionRow {
            id: "sess-1".into(),
            source: "claude".into(),
            project_id: "proj-1".into(),
            name: Some("test session".into()),
            mode: None,
            model: None,
            created_at: Some(chrono::Utc::now().timestamp()),
            updated_at: Some(chrono::Utc::now().timestamp()),
            message_count: 10,
            first_message: None,
            indexed_at: chrono::Utc::now().timestamp(),
        })
        .unwrap();
        db.upsert_stats(&StatsRow {
            session_id: "sess-1".into(),
            source: "claude".into(),
            model: Some("claude-opus-4-5".into()),
            input_tokens: Some(25000),
            output_tokens: Some(12000),
            cache_read_tokens: Some(5000),
            cache_creation_tokens: Some(1000),
            is_estimated: false,
            token_source: "native".into(),
            duration_secs: Some(180),
            files_touched: vec!["src/main.rs".into(), "src/lib.rs".into()],
            tool_call_count: 7,
            computed_at: chrono::Utc::now().timestamp(),
        })
        .unwrap();
        db
    }

    #[test]
    fn test_json_output_structure() {
        let db = seed_db();
        let stats = db.aggregate_stats_global().unwrap();

        let per_tool: Vec<serde_json::Value> = db
            .aggregate_stats_per_tool()
            .unwrap_or_default()
            .iter()
            .map(|(source, s)| {
                serde_json::json!({
                    "tool": source,
                    "sessions": s.session_count,
                    "input_tokens": s.total_input_tokens,
                    "output_tokens": s.total_output_tokens,
                })
            })
            .collect();

        let json = serde_json::json!({
            "scope": { "project": serde_json::Value::Null, "tool": serde_json::Value::Null },
            "sessions": stats.session_count,
            "input_tokens": stats.total_input_tokens,
            "output_tokens": stats.total_output_tokens,
            "total_tokens": stats.total_input_tokens + stats.total_output_tokens,
            "total_duration_secs": stats.total_duration_secs,
            "per_tool": per_tool,
        });

        let json_str = serde_json::to_string_pretty(&json).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert!(parsed.is_object());
        assert_eq!(parsed["sessions"], 1);
        assert_eq!(parsed["input_tokens"], 25000);
        assert_eq!(parsed["output_tokens"], 12000);
        assert_eq!(parsed["total_tokens"], 37000);
        assert_eq!(parsed["per_tool"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["per_tool"][0]["tool"], "claude");
    }

    #[test]
    fn test_json_output_empty_db() {
        let db = Db::open_in_memory().unwrap();
        let stats = db.aggregate_stats_global().unwrap();

        let json = serde_json::json!({
            "sessions": stats.session_count,
            "input_tokens": stats.total_input_tokens,
            "output_tokens": stats.total_output_tokens,
        });
        let json_str = serde_json::to_string_pretty(&json).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["sessions"], 0);
        assert_eq!(parsed["input_tokens"], 0);
        assert_eq!(parsed["output_tokens"], 0);
    }

    #[test]
    fn test_build_data_sources_labels() {
        let per_tool = vec![
            (
                "claude".to_string(),
                crate::db::stats::AggregateStats::default(),
            ),
            (
                "composer".to_string(),
                crate::db::stats::AggregateStats::default(),
            ),
            (
                "gemini".to_string(),
                crate::db::stats::AggregateStats::default(),
            ),
        ];
        let sources = build_data_sources(&per_tool);
        assert_eq!(sources.len(), 3);
        assert_eq!(sources[0].0, "Claude");
        assert_eq!(sources[1].0, "Cursor");
        assert_eq!(sources[2].0, "Gemini CLI");
    }
}
