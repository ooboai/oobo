use std::collections::HashMap;

use chrono::Datelike;

use crate::db::Db;
use crate::tui;

pub fn run(agent: bool, out: Option<String>, format: &str) -> Result<(), String> {
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
    let weekly = db.weekly_ai_trend(12).unwrap_or_default();

    let author = git_author();
    let earliest = earliest_session(&db);
    let tool_count = per_tool.len();
    let project_count = projects.len();
    let total_tokens = global.total_input_tokens + global.total_output_tokens;

    let heatmap = build_heatmap(&db);
    let ai_streak = compute_ai_streak(&heatmap);

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
        ai_streak,
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
        weekly_trend: {
            let mut wt: Vec<WeekEntry> = weekly
                .iter()
                .map(|w| WeekEntry {
                    week: w.week.clone(),
                    ai_pct: w.ai_percentage(),
                })
                .collect();
            wt.reverse();
            wt
        },
        heatmap,
    };

    match format {
        "json" => {
            print_json(&card);
            return Ok(());
        }
        "md" => {
            if !agent {
                print_terminal(&card);
            }
            let md = render_markdown(&card);
            let path = out.unwrap_or_else(|| "oobo-card.md".to_string());
            std::fs::write(&path, &md).map_err(|e| format!("cannot write {path}: {e}"))?;
            if !agent {
                eprintln!("\n  saved to {path}");
            }
            return Ok(());
        }
        _ => {}
    }

    let is_svg = format == "svg";

    if agent && !is_svg {
        print_json(&card);
    } else if !agent {
        print_terminal(&card);
    }

    if agent && out.is_none() && !is_svg {
        return Ok(());
    }

    let svg = render_svg(&card);

    if is_svg {
        let path = out.unwrap_or_else(|| "oobo-card.svg".to_string());
        std::fs::write(&path, &svg).map_err(|e| format!("cannot write {path}: {e}"))?;
        if !agent {
            eprintln!("\n  saved to {path}");
        }
    } else {
        let png = svg_to_png(&svg)?;
        let path = out.unwrap_or_else(|| "oobo-card.png".to_string());
        std::fs::write(&path, &png).map_err(|e| format!("cannot write {path}: {e}"))?;
        if !agent {
            eprintln!("\n  saved to {path}");
        }
    }

    Ok(())
}

// ── Data structures ──────────────────────────────────────────────────────────

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
    ai_streak: i64,
    top_tools: Vec<ToolEntry>,
    top_models: Vec<ModelEntry>,
    weekly_trend: Vec<WeekEntry>,
    heatmap: Vec<DayCell>,
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

#[derive(Clone)]
struct DayCell {
    date: chrono::NaiveDate,
    commits: i64,
    ai_assisted: i64,
}

#[allow(dead_code)]
mod colors {
    pub const PRIMARY: &str = "#1F4F6D";
    pub const CYAN: &str = "#7DD3E8";
    pub const TEAL: &str = "#14b8a6";
    pub const BG: &str = "#f8fafb";
    pub const DARK: &str = "#111111";
    pub const BORDER: &str = "#D9E3E1";
    pub const SECONDARY: &str = "#5F7F84";
    pub const EMPTY_CELL: &str = "#1a3f57";
    pub const MUTED: &str = "#2a6485";
    pub const MUTED_LIGHT: &str = "#4a7a94";
    pub const TEAL_SHADES: [&str; 4] = ["#b2dfdb", "#4db6ac", "#14b8a6", "#0d7d72"];
    pub const CYAN_SHADES: [&str; 4] = ["#c5edf5", "#7DD3E8", "#4bbcd6", "#2a98b3"];
}
use colors::*;

// ── Heatmap data ─────────────────────────────────────────────────────────────

fn build_heatmap(db: &Db) -> Vec<DayCell> {
    let today = chrono::Local::now().date_naive();
    let start = today - chrono::Duration::days(364);

    let mut day_map: HashMap<String, (i64, i64)> = HashMap::new();

    if let Ok(mut stmt) = db.conn.prepare(
        "SELECT date, SUM(commits), SUM(ai_assisted_commits)
         FROM git_activity
         WHERE date >= ?1
         GROUP BY date
         ORDER BY date",
    ) {
        let start_str = start.format("%Y-%m-%d").to_string();
        if let Ok(rows) = stmt.query_map(rusqlite::params![start_str], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        }) {
            for r in rows.flatten() {
                day_map.insert(r.0, (r.1, r.2));
            }
        }
    }

    let mut cells = Vec::with_capacity(365);
    let mut d = start;
    while d <= today {
        let key = d.format("%Y-%m-%d").to_string();
        let (commits, ai) = day_map.get(&key).copied().unwrap_or((0, 0));
        cells.push(DayCell {
            date: d,
            commits,
            ai_assisted: ai,
        });
        d += chrono::Duration::days(1);
    }
    cells
}

/// Current streak of consecutive days with AI-assisted commits (from today backwards).
fn compute_ai_streak(heatmap: &[DayCell]) -> i64 {
    let mut streak: i64 = 0;
    for day in heatmap.iter().rev() {
        if day.ai_assisted > 0 {
            streak += 1;
        } else if day.commits > 0 {
            break;
        } else {
            // skip days with zero activity (weekends, gaps) -- don't break the streak
            // but only if the streak already started
            if streak > 0 {
                continue;
            }
            // haven't started yet, keep scanning back
        }
    }
    streak
}

// ── SVG renderer ─────────────────────────────────────────────────────────────

fn render_svg(card: &CardData) -> String {
    let w = 880;
    let font = "Helvetica, Arial, sans-serif";
    let pad = 32;

    let mut svg_body = String::with_capacity(16384);
    let mut y: i32 = 0;

    // ── Header: name + stats row ────────────────────────────────────────
    y += pad;
    svg_body.push_str(&format!(
        r#"<text x="{pad}" y="{y}" font-size="20" font-weight="bold" fill="white">{}</text>"#,
        xml_escape(&card.author)
    ));

    // Inline stats on the right side of the header
    let stats_right = w - pad;
    let streak_str = if card.ai_streak > 0 {
        format!("{}d", card.ai_streak)
    } else {
        "—".to_string()
    };
    let stat_vals: Vec<(String, &str)> = vec![
        (format_number(card.ai_commits), "commits"),
        (tui::format_tokens(card.total_tokens), "tokens"),
        (format!("{}", card.session_count), "sessions"),
        (format!("{}", card.active_days), "days"),
        (streak_str, "streak"),
    ];
    let mut sx = stats_right;
    for (val, label) in stat_vals.iter().rev() {
        svg_body.push_str(&format!(
            r#"<text x="{sx}" y="{}" font-size="9" fill="{MUTED_LIGHT}" text-anchor="end">{label}</text>"#,
            y
        ));
        sx -= (label.len() as i32) * 5 + 6;
        svg_body.push_str(&format!(
            r#"<text x="{sx}" y="{}" font-size="12" font-weight="bold" fill="white" text-anchor="end">{val}</text>"#,
            y
        ));
        sx -= (val.len() as i32) * 7 + 16;
    }

    y += 12;
    svg_body.push_str(&format!(
        r#"<line x1="{pad}" y1="{y}" x2="{}" y2="{y}" stroke="{MUTED}" stroke-width="0.5" opacity="0.5"/>"#,
        w - pad
    ));

    // ── Contribution grid (full year, 52 weeks x 7 days) ────────────────
    y += 18;

    let cell = 11;
    let gap = 3;
    let step = cell + gap;
    let grid_left = pad + 28;

    let max_commits = card.heatmap.iter().map(|c| c.commits).max().unwrap_or(1).max(1);

    // Month labels
    if !card.heatmap.is_empty() {
        let mut last_month = 0u32;
        for c in &card.heatmap {
            let m = c.date.month();
            if m != last_month {
                let day_offset = (c.date - card.heatmap[0].date).num_days();
                let week_col = day_offset / 7;
                let x = grid_left as i64 + week_col * step as i64;
                svg_body.push_str(&format!(
                    r#"<text x="{x}" y="{}" font-size="10" fill="{MUTED_LIGHT}">{}</text>"#,
                    y, c.date.format("%b")
                ));
                last_month = m;
            }
        }
    }
    y += 8;

    // Day labels
    for (i, label) in ["", "Mon", "", "Wed", "", "Fri", ""].iter().enumerate() {
        if !label.is_empty() {
            svg_body.push_str(&format!(
                r#"<text x="{pad}" y="{}" font-size="9" fill="{MUTED_LIGHT}">{label}</text>"#,
                y + (i as i32) * step + cell
            ));
        }
    }

    // Grid cells
    let first_wd = card
        .heatmap
        .first()
        .map(|c| c.date.weekday().num_days_from_sunday())
        .unwrap_or(0);
    for (i, day) in card.heatmap.iter().enumerate() {
        let offset = i as u32 + first_wd;
        let col = offset / 7;
        let row = offset % 7;
        let cx = grid_left as u32 + col * step as u32;
        let cy = y as u32 + row * step as u32;
        let color = heatmap_color(day, max_commits);
        svg_body.push_str(&format!(
            r#"<rect x="{cx}" y="{cy}" width="{cell}" height="{cell}" rx="3" fill="{color}"/>"#
        ));
    }
    y += 7 * step + 6;

    // Legend + AI% badge (right side, same line)
    let legend_y = y + 2;

    // AI percentage badge
    let ai_pct = card.ai_percentage.unwrap_or(0.0);
    svg_body.push_str(&format!(
        r#"<rect x="{pad}" y="{}" width="64" height="20" rx="10" fill="{CYAN}" opacity="0.15"/>"#,
        legend_y - 14
    ));
    svg_body.push_str(&format!(
        r#"<text x="{}" y="{}" font-size="10" font-weight="bold" fill="{CYAN}" text-anchor="middle">{ai_pct:.0}% AI</text>"#,
        pad + 32, legend_y
    ));

    // Legend squares
    let mut lx = w - pad;
    for (label, color) in [("AI-assisted", CYAN), ("Human", TEAL)].iter().rev() {
        let tw = label.len() as i32 * 6;
        lx -= tw;
        svg_body.push_str(&format!(
            r#"<text x="{lx}" y="{legend_y}" font-size="9" fill="{MUTED_LIGHT}">{label}</text>"#
        ));
        lx -= cell + 5;
        svg_body.push_str(&format!(
            r#"<rect x="{lx}" y="{}" width="{cell}" height="{cell}" rx="3" fill="{color}"/>"#,
            legend_y - 10
        ));
        lx -= 14;
    }
    // Less/More gradient
    lx -= 6;
    svg_body.push_str(&format!(
        r#"<text x="{lx}" y="{legend_y}" font-size="9" fill="{MUTED_LIGHT}" text-anchor="end">More</text>"#
    ));
    lx -= 30;
    for c in [TEAL_SHADES[3], TEAL_SHADES[2], TEAL_SHADES[1], TEAL_SHADES[0], EMPTY_CELL].iter() {
        svg_body.push_str(&format!(
            r#"<rect x="{lx}" y="{}" width="{cell}" height="{cell}" rx="3" fill="{c}"/>"#,
            legend_y - 10
        ));
        lx -= cell + 2;
    }
    svg_body.push_str(&format!(
        r#"<text x="{}" y="{legend_y}" font-size="9" fill="{MUTED_LIGHT}" text-anchor="end">Less</text>"#,
        lx - 2
    ));

    y = legend_y + 36;

    // ── Tools strip ─────────────────────────────────────────────────────
    svg_body.push_str(&format!(
        r#"<line x1="{pad}" y1="{y}" x2="{}" y2="{y}" stroke="{MUTED}" stroke-width="0.5" opacity="0.4"/>"#,
        w - pad
    ));
    y += 20;

    let tool_count = card.top_tools.len().min(5);
    if tool_count > 0 {
        let col_w = (w - 2 * pad) / tool_count as i32;
        for (i, t) in card.top_tools.iter().take(tool_count).enumerate() {
            let tx = pad + i as i32 * col_w;
            svg_body.push_str(&format!(
                r#"<text x="{tx}" y="{y}" font-size="12" font-weight="bold" fill="white">{}</text>"#,
                xml_escape(&t.name)
            ));
            svg_body.push_str(&format!(
                r#"<text x="{tx}" y="{}" font-size="9" fill="{MUTED_LIGHT}">{} · {}</text>"#,
                y + 14, t.sessions, tui::format_tokens(t.tokens)
            ));
        }
    }
    y += 32;

    // ── Footer ──────────────────────────────────────────────────────────
    svg_body.push_str(&format!(
        r#"<text x="{}" y="{y}" font-size="8" fill="{MUTED}" text-anchor="end">oobo.ai</text>"#,
        w - pad
    ));
    y += 12;

    // ── Assemble ────────────────────────────────────────────────────────
    let h = y;
    let mut svg = String::with_capacity(svg_body.len() + 512);
    svg.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" font-family="{font}">"#
    ));
    svg.push_str(&format!(
        r#"<rect width="{w}" height="{h}" rx="12" fill="{PRIMARY}"/>"#
    ));
    svg.push_str(&svg_body);
    svg.push_str("</svg>\n");
    svg
}

fn heatmap_color(cell: &DayCell, max_commits: i64) -> &'static str {
    if cell.commits == 0 {
        return EMPTY_CELL;
    }
    let intensity = (cell.commits as f64 / max_commits as f64).min(1.0);
    let bucket = match intensity {
        x if x <= 0.25 => 0,
        x if x <= 0.50 => 1,
        x if x <= 0.75 => 2,
        _ => 3,
    };

    if cell.ai_assisted > 0 {
        CYAN_SHADES[bucket]
    } else {
        TEAL_SHADES[bucket]
    }
}

fn svg_to_png(svg_data: &str) -> Result<Vec<u8>, String> {
    let scale = 4.0_f32;

    let mut opts = resvg::usvg::Options {
        font_family: "Helvetica".to_string(),
        ..Default::default()
    };
    opts.fontdb_mut().load_system_fonts();

    let tree = resvg::usvg::Tree::from_str(svg_data, &opts)
        .map_err(|e| format!("SVG parse error: {e}"))?;

    let size = tree.size();
    let w = (size.width() * scale).ceil() as u32;
    let h = (size.height() * scale).ceil() as u32;

    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(w, h).ok_or("cannot create pixmap")?;

    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    pixmap
        .encode_png()
        .map_err(|e| format!("PNG encode error: {e}"))
}

fn format_number(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{n}")
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ── Helpers ──────────────────────────────────────────────────────────────────

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

// ── Terminal output ──────────────────────────────────────────────────────────

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

// ── Markdown output ──────────────────────────────────────────────────────────

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

// ── JSON output ──────────────────────────────────────────────────────────────

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

    let svg = render_svg(c);

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
        "svg": svg,
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
    fn test_xml_escape() {
        assert_eq!(xml_escape("a<b>c&d"), "a&lt;b&gt;c&amp;d");
    }

    #[test]
    fn test_heatmap_color_empty() {
        let cell = DayCell {
            date: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            commits: 0,
            ai_assisted: 0,
        };
        assert_eq!(heatmap_color(&cell, 10), EMPTY_CELL);
    }

    #[test]
    fn test_heatmap_color_human() {
        let cell = DayCell {
            date: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            commits: 5,
            ai_assisted: 0,
        };
        let color = heatmap_color(&cell, 10);
        assert!(TEAL_SHADES.contains(&color));
    }

    #[test]
    fn test_heatmap_color_ai() {
        let cell = DayCell {
            date: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            commits: 5,
            ai_assisted: 3,
        };
        let color = heatmap_color(&cell, 10);
        assert!(CYAN_SHADES.contains(&color));
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
            ai_streak: 5,
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
            heatmap: vec![],
        };

        let md = render_markdown(&card);
        assert!(md.contains("# TestDev"));
        assert!(md.contains("| AI Tools | 3 active |"));
        assert!(md.contains("| Cursor | 30 | 300.0K |"));
        assert!(md.contains("| sonnet-4 | 25 | 60.0% |"));
        assert!(md.contains("2026-01-01"));
    }

    #[test]
    fn test_render_svg_structure() {
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
            ai_streak: 5,
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
            weekly_trend: vec![],
            heatmap: vec![],
        };

        let svg = render_svg(&card);
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("TestDev"));
        assert!(svg.contains("oobo.ai"));
        assert!(svg.contains("100"));
        assert!(svg.contains("45%"));
        assert!(svg.contains("500.0K"));
        assert!(svg.contains("Cursor"));
        assert!(svg.ends_with("</svg>\n"));
    }
}
