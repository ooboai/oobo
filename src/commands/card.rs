use crate::db::Db;
use crate::tui;

pub fn run(json: bool, out: Option<String>) -> Result<(), String> {
    let db = Db::open()?;

    let global = db.aggregate_stats_global()?;
    if global.session_count == 0 {
        eprintln!("no data yet. Run `oobo scan` then `oobo index` first.");
        return Ok(());
    }

    let per_tool = db.aggregate_stats_per_tool()?;
    let per_model = db.aggregate_stats_by_model()?;
    let ai_headline = db.ai_code_percentage(None, None).ok();
    let productivity = db.productivity_summary().ok();
    let projects = db.list_projects()?;
    let weekly = db.weekly_ai_trend(4).unwrap_or_default();

    let author = git_author();
    let earliest = earliest_session(&db);
    let tool_count = per_tool.len();
    let project_count = projects.len();
    let total_tokens = global.total_input_tokens + global.total_output_tokens;

    let card = CardData {
        author,
        tool_count,
        project_count,
        session_count: global.session_count,
        total_tokens,
        ai_percentage: ai_headline.as_ref().map(|a| a.ai_percentage),
        ai_commits: ai_headline.as_ref().map(|a| a.total_commits).unwrap_or(0),
        commits_per_day: productivity.as_ref().map(|p| p.commits_per_day()),
        active_days: productivity.as_ref().map(|p| p.active_days).unwrap_or(0),
        active_since: earliest,
        top_tools: per_tool
            .iter()
            .take(5)
            .map(|(source, s)| ToolEntry {
                name: tui::source_label(source).to_string(),
                sessions: s.session_count,
                tokens: s.total_input_tokens + s.total_output_tokens,
            })
            .collect(),
        top_models: per_model
            .iter()
            .take(3)
            .map(|m| ModelEntry {
                name: shorten_model(&m.model),
                sessions: m.session_count,
                pct: m.pct_of_total_output,
            })
            .collect(),
        weekly_trend: weekly
            .iter()
            .map(|w| WeekEntry {
                week: w.week.clone(),
                ai_pct: w.ai_percentage(),
            })
            .collect(),
    };

    if json {
        print_json(&card);
    } else {
        print_terminal(&card);
    }

    let md = render_markdown(&card);
    let path = out.unwrap_or_else(|| "oobo-card.md".to_string());
    std::fs::write(&path, &md).map_err(|e| format!("cannot write {path}: {e}"))?;

    if !json {
        eprintln!("\n  saved to {path}");
    }

    Ok(())
}

struct CardData {
    author: String,
    tool_count: usize,
    project_count: usize,
    session_count: i64,
    total_tokens: i64,
    ai_percentage: Option<f64>,
    ai_commits: i64,
    commits_per_day: Option<f64>,
    active_days: i64,
    active_since: Option<String>,
    top_tools: Vec<ToolEntry>,
    top_models: Vec<ModelEntry>,
    weekly_trend: Vec<WeekEntry>,
}

struct ToolEntry {
    name: String,
    sessions: i64,
    tokens: i64,
}

struct ModelEntry {
    name: String,
    sessions: i64,
    pct: f64,
}

struct WeekEntry {
    week: String,
    ai_pct: f64,
}

fn git_author() -> String {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let name = std::process::Command::new(git)
        .args(["config", "user.name"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    if name.is_empty() {
        "Developer".to_string()
    } else {
        name
    }
}

fn earliest_session(db: &Db) -> Option<String> {
    db.conn
        .query_row(
            "SELECT MIN(created_at) FROM sessions WHERE created_at IS NOT NULL AND created_at > 0",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()
        .and_then(|ts| {
            let ts = crate::utils::to_epoch_secs(ts);
            chrono::DateTime::from_timestamp(ts, 0).map(|dt| dt.format("%Y-%m-%d").to_string())
        })
}

fn shorten_model(s: &str) -> String {
    let s = s
        .replace("claude-", "")
        .replace("gpt-", "GPT-")
        .replace("gemini-", "Gemini ");
    if s.chars().count() > 20 {
        let truncated: String = s.chars().take(19).collect();
        format!("{truncated}…")
    } else {
        s
    }
}

fn print_terminal(c: &CardData) {
    let ai_pct_str = c
        .ai_percentage
        .map(|p| format!("{:.0}%", p))
        .unwrap_or_else(|| "n/a".to_string());

    eprintln!();
    eprintln!("  \x1b[1m{}\x1b[0m", c.author);
    eprintln!();

    let kv = |label: &str, value: &str| {
        eprintln!("  \x1b[90m{:<16}\x1b[0m {}", label, value);
    };

    kv("tools", &format!("{}", c.tool_count));
    kv("sessions", &format!("{}", c.session_count));
    kv("tokens", &tui::format_tokens(c.total_tokens));
    kv("ai code", &ai_pct_str);
    kv("projects", &format!("{}", c.project_count));

    if !c.top_tools.is_empty() {
        eprintln!();
        eprintln!("  \x1b[1mtools\x1b[0m");
        for t in &c.top_tools {
            eprintln!(
                "  \x1b[90m{:<16}\x1b[0m {:>4} sessions   {:>8} tokens",
                t.name,
                t.sessions,
                tui::format_tokens(t.tokens)
            );
        }
    }

    if !c.top_models.is_empty() {
        eprintln!();
        eprintln!("  \x1b[1mmodels\x1b[0m");
        for m in &c.top_models {
            eprintln!(
                "  \x1b[90m{:<20}\x1b[0m {:>4} sessions   {:>5.1}%",
                m.name, m.sessions, m.pct
            );
        }
    }

    if c.ai_commits > 0 {
        eprintln!();
        eprintln!("  \x1b[1mcommits\x1b[0m");
        if let Some(cpd) = c.commits_per_day {
            eprintln!("  \x1b[90m{:<16}\x1b[0m {:.1}", "per day", cpd);
        }
        eprintln!("  \x1b[90m{:<16}\x1b[0m {}", "active days", c.active_days);
        eprintln!(
            "  \x1b[90m{:<16}\x1b[0m {} ({})",
            "ai-assisted", c.ai_commits, ai_pct_str
        );
    }

    if !c.weekly_trend.is_empty() {
        eprintln!();
        eprintln!("  \x1b[1mai code trend\x1b[0m");
        for w in &c.weekly_trend {
            let bar_len = (w.ai_pct / 5.0).round() as usize;
            let bar: String = "█".repeat(bar_len.min(20));
            eprintln!(
                "  \x1b[90m{:<10}\x1b[0m {:>5.1}%  {}",
                w.week, w.ai_pct, bar
            );
        }
    }

    eprintln!();
    if let Some(ref since) = c.active_since {
        eprintln!(
            "  \x1b[90mtracking since {}  oobo v{}\x1b[0m",
            since,
            env!("CARGO_PKG_VERSION")
        );
    } else {
        eprintln!("  \x1b[90moobo v{}\x1b[0m", env!("CARGO_PKG_VERSION"));
    }
    eprintln!();
}

fn render_markdown(c: &CardData) -> String {
    let ai_pct_str = c
        .ai_percentage
        .map(|p| format!("{:.0}%", p))
        .unwrap_or_else(|| "n/a".to_string());

    let mut md = String::new();
    md.push_str(&format!("# {}\n\n", c.author));
    md.push_str(&format!(
        "*Generated by [oobo](https://github.com/ooboai/oobo) on {}*\n\n",
        chrono::Local::now().format("%Y-%m-%d")
    ));

    md.push_str("## Overview\n\n");
    md.push_str("| Metric | Value |\n");
    md.push_str("|--------|-------|\n");
    md.push_str(&format!("| AI Tools | {} active |\n", c.tool_count));
    md.push_str(&format!("| Sessions | {} |\n", c.session_count));
    md.push_str(&format!(
        "| Tokens | {} |\n",
        tui::format_tokens(c.total_tokens)
    ));
    md.push_str(&format!("| AI-Written Code | {} |\n", ai_pct_str));
    md.push_str(&format!("| Projects | {} |\n", c.project_count));
    if c.active_days > 0 {
        md.push_str(&format!("| Active Days | {} |\n", c.active_days));
    }

    if !c.top_tools.is_empty() {
        md.push_str("\n## Tools\n\n");
        md.push_str("| Tool | Sessions | Tokens |\n");
        md.push_str("|------|----------|--------|\n");
        for t in &c.top_tools {
            md.push_str(&format!(
                "| {} | {} | {} |\n",
                t.name,
                t.sessions,
                tui::format_tokens(t.tokens)
            ));
        }
    }

    if !c.top_models.is_empty() {
        md.push_str("\n## Models\n\n");
        md.push_str("| Model | Sessions | Share |\n");
        md.push_str("|-------|----------|-------|\n");
        for m in &c.top_models {
            md.push_str(&format!(
                "| {} | {} | {:.1}% |\n",
                m.name, m.sessions, m.pct
            ));
        }
    }

    if c.ai_commits > 0 {
        md.push_str("\n## Commit Profile\n\n");
        md.push_str("| Metric | Value |\n");
        md.push_str("|--------|-------|\n");
        md.push_str(&format!("| AI-Assisted Commits | {} |\n", c.ai_commits));
        md.push_str(&format!("| AI Code | {} |\n", ai_pct_str));
        if let Some(cpd) = c.commits_per_day {
            md.push_str(&format!("| Commits/Day | {:.1} |\n", cpd));
        }
    }

    if !c.weekly_trend.is_empty() {
        md.push_str("\n## AI Code Trend\n\n");
        md.push_str("| Week | AI % |\n");
        md.push_str("|------|------|\n");
        for w in &c.weekly_trend {
            md.push_str(&format!("| {} | {:.0}% |\n", w.week, w.ai_pct));
        }
    }

    md.push_str("\n---\n\n");
    if let Some(ref since) = c.active_since {
        md.push_str(&format!(
            "*Tracking since {} · oobo v{}*\n",
            since,
            env!("CARGO_PKG_VERSION")
        ));
    } else {
        md.push_str(&format!("*oobo v{}*\n", env!("CARGO_PKG_VERSION")));
    }

    md
}

fn print_json(c: &CardData) {
    let tools: Vec<serde_json::Value> = c
        .top_tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "tool": t.name,
                "sessions": t.sessions,
                "tokens": t.tokens,
            })
        })
        .collect();

    let models: Vec<serde_json::Value> = c
        .top_models
        .iter()
        .map(|m| {
            serde_json::json!({
                "model": m.name,
                "sessions": m.sessions,
                "share_pct": m.pct,
            })
        })
        .collect();

    let trend: Vec<serde_json::Value> = c
        .weekly_trend
        .iter()
        .map(|w| {
            serde_json::json!({
                "week": w.week,
                "ai_pct": w.ai_pct,
            })
        })
        .collect();

    let json = serde_json::json!({
        "author": c.author,
        "tool_count": c.tool_count,
        "project_count": c.project_count,
        "session_count": c.session_count,
        "total_tokens": c.total_tokens,
        "ai_code_percentage": c.ai_percentage,
        "ai_commits": c.ai_commits,
        "commits_per_day": c.commits_per_day,
        "active_days": c.active_days,
        "active_since": c.active_since,
        "tools": tools,
        "models": models,
        "weekly_trend": trend,
        "oobo_version": env!("CARGO_PKG_VERSION"),
        "generated_at": chrono::Utc::now().to_rfc3339(),
    });

    crate::utils::print_json(&json);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shorten_model_short() {
        assert_eq!(shorten_model("gpt-4o"), "GPT-4o");
    }

    #[test]
    fn test_shorten_model_claude() {
        assert_eq!(shorten_model("claude-sonnet-4"), "sonnet-4");
    }

    #[test]
    fn test_shorten_model_gemini() {
        assert_eq!(shorten_model("gemini-2.5-pro"), "Gemini 2.5-pro");
    }

    #[test]
    fn test_shorten_model_long() {
        let long = "claude-sonnet-4-6-20260301-extra-long-suffix";
        let result = shorten_model(long);
        assert!(result.chars().count() <= 20);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_git_author_returns_something() {
        let author = git_author();
        assert!(!author.is_empty());
    }

    #[test]
    fn test_render_markdown_structure() {
        let card = CardData {
            author: "TestDev".to_string(),
            tool_count: 3,
            project_count: 5,
            session_count: 42,
            total_tokens: 500_000,
            ai_percentage: Some(45.0),
            ai_commits: 100,
            commits_per_day: Some(3.5),
            active_days: 30,
            active_since: Some("2026-01-01".to_string()),
            top_tools: vec![ToolEntry {
                name: "Cursor".to_string(),
                sessions: 30,
                tokens: 300_000,
            }],
            top_models: vec![ModelEntry {
                name: "sonnet-4".to_string(),
                sessions: 25,
                pct: 60.0,
            }],
            weekly_trend: vec![WeekEntry {
                week: "2026-W10".to_string(),
                ai_pct: 48.0,
            }],
        };

        let md = render_markdown(&card);
        assert!(md.contains("# TestDev"));
        assert!(md.contains("| AI Tools | 3 active |"));
        assert!(md.contains("| Cursor | 30 | 300.0K |"));
        assert!(md.contains("| sonnet-4 | 25 | 60.0% |"));
        assert!(md.contains("2026-01-01"));
    }
}
