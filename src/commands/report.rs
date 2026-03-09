use crate::db::Db;

pub fn run(format: &str) -> Result<(), String> {
    let db = Db::open()?;

    let global = db.aggregate_stats_global()?;
    let per_tool = db.aggregate_stats_per_tool()?;
    let per_model = db.aggregate_stats_by_model()?;
    let ai_headline = db.ai_code_percentage(None, None)?;
    let productivity = db.productivity_summary()?;
    let ai_weekly = db.weekly_ai_trend(4)?;

    if format == "json" {
        print_json(
            &global,
            &per_tool,
            &per_model,
            &ai_headline,
            &productivity,
            &ai_weekly,
        );
    } else {
        print_text(
            &global,
            &per_tool,
            &per_model,
            &ai_headline,
            &productivity,
            &ai_weekly,
        );
    }

    Ok(())
}

fn print_text(
    global: &crate::db::stats::AggregateStats,
    per_tool: &[(String, crate::db::stats::AggregateStats)],
    per_model: &[crate::db::stats::ModelStats],
    ai: &crate::db::ai_commits::AiCodeHeadline,
    prod: &crate::analytics::git_activity::ProductivitySummary,
    weekly: &[crate::db::ai_commits::AiWeeklyTrend],
) {
    let total_tokens = global.total_input_tokens + global.total_output_tokens;

    println!("╔══════════════════════════════════════════════════╗");
    println!("║              oobo monthly report                 ║");
    println!("╚══════════════════════════════════════════════════╝");
    println!();

    if ai.total_commits > 0 {
        println!(
            "  AI Code:    {:.1}% AI-assisted ({} commits, {} lines)",
            ai.ai_percentage,
            ai.total_commits,
            fmt_num(ai.total_lines)
        );
    }

    println!(
        "  Sessions:   {} across {} tools",
        global.session_count,
        per_tool.len()
    );
    println!(
        "  Tokens:     {} ({} in / {} out)",
        crate::tui::format_tokens(total_tokens),
        crate::tui::format_tokens(global.total_input_tokens),
        crate::tui::format_tokens(global.total_output_tokens),
    );
    if global.total_duration_secs > 0 {
        println!(
            "  Time in AI: {}",
            crate::tui::format_duration(global.total_duration_secs)
        );
    }

    if prod.total_commits > 0 {
        println!();
        println!("  Git Productivity:");
        println!(
            "    {:.1} commits/day, {:.0} lines/day across {} active days",
            prod.commits_per_day(),
            prod.lines_per_day(),
            prod.active_days
        );
    }

    if !per_tool.is_empty() {
        println!();
        println!("  Per Tool:");
        for (source, s) in per_tool {
            let label = crate::tui::source_label(source);
            println!(
                "    {:<12} {:>5} sessions  {:>8} tokens",
                label,
                s.session_count,
                crate::tui::format_tokens(s.total_input_tokens + s.total_output_tokens),
            );
        }
    }

    if !per_model.is_empty() {
        println!();
        println!("  Top Models:");
        for m in per_model.iter().take(5) {
            println!(
                "    {:<24} {:>5} sessions  {:>8} tokens  {:.1}%",
                shorten(&m.model, 24),
                m.session_count,
                crate::tui::format_tokens(m.input_tokens + m.output_tokens),
                m.pct_of_total_output,
            );
        }
    }

    if !weekly.is_empty() {
        println!();
        println!("  AI Code Trend:");
        for w in weekly.iter().take(4) {
            let pct = w.ai_percentage();
            println!(
                "    {:<12} {:.0}% AI  ({} commits, {} lines)",
                w.week,
                pct,
                w.commits,
                fmt_num(w.lines_added),
            );
        }
    }

    println!();
}

fn print_json(
    global: &crate::db::stats::AggregateStats,
    per_tool: &[(String, crate::db::stats::AggregateStats)],
    per_model: &[crate::db::stats::ModelStats],
    ai: &crate::db::ai_commits::AiCodeHeadline,
    prod: &crate::analytics::git_activity::ProductivitySummary,
    weekly: &[crate::db::ai_commits::AiWeeklyTrend],
) {
    let tools: Vec<serde_json::Value> = per_tool
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

    let models: Vec<serde_json::Value> = per_model
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

    let weekly_trend: Vec<serde_json::Value> = weekly
        .iter()
        .map(|w| {
            serde_json::json!({
                "week": w.week,
                "commits": w.commits,
                "lines_added": w.lines_added,
                "ai_lines": w.ai_lines,
                "human_lines": w.human_lines,
                "ai_percentage": w.ai_percentage(),
            })
        })
        .collect();

    let json = serde_json::json!({
        "summary": {
            "sessions": global.session_count,
            "input_tokens": global.total_input_tokens,
            "output_tokens": global.total_output_tokens,
            "total_tokens": global.total_input_tokens + global.total_output_tokens,
            "duration_secs": global.total_duration_secs,
        },
        "ai_code": {
            "total_commits": ai.total_commits,
            "total_lines": ai.total_lines,
            "ai_lines": ai.ai_lines,
            "human_lines": ai.human_lines,
            "ai_percentage": ai.ai_percentage,
        },
        "productivity": {
            "active_days": prod.active_days,
            "total_commits": prod.total_commits,
            "lines_added": prod.total_lines_added,
            "lines_deleted": prod.total_lines_deleted,
            "commits_per_day": prod.commits_per_day(),
            "lines_per_day": prod.lines_per_day(),
        },
        "per_tool": tools,
        "per_model": models,
        "ai_weekly_trend": weekly_trend,
    });

    crate::utils::print_json(&json);
}

fn fmt_num(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn shorten(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max - 1])
    }
}
