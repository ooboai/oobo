use std::collections::HashMap;

use chrono::Datelike;

use crate::cli::OutputMode;
use crate::db::Db;
use crate::tui;

pub fn run(mode: OutputMode, out: Option<String>, format: &str) -> Result<(), String> {
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
        ai_percentage: ai_headline.as_ref().and_then(|a| {
            if a.total_commits > 0 {
                Some(a.ai_percentage)
            } else {
                None
            }
        }),
        ai_commits: db.ai_commit_count().unwrap_or(0),
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

    let is_structured = mode.is_structured();

    match format {
        "json" => {
            print_json(&card, true);
            return Ok(());
        }
        "md" => {
            if !is_structured {
                print_terminal(&card);
            }
            let md = render_markdown(&card);
            let path = out.unwrap_or_else(|| "oobo-card.md".to_string());
            std::fs::write(&path, &md).map_err(|e| format!("cannot write {path}: {e}"))?;
            if is_structured {
                println!("saved: {path}");
            } else {
                eprintln!("\n  saved to {path}");
            }
            return Ok(());
        }
        _ => {}
    }

    let is_svg = format == "svg";

    if is_structured && !is_svg {
        match mode {
            OutputMode::Agent => print_agent_compact(&card),
            _ => print_json(&card, false),
        }
    } else if !is_structured {
        print_terminal(&card);
    }

    if is_structured && out.is_none() && !is_svg {
        return Ok(());
    }

    let svg = render_svg(&card);

    if is_svg {
        let path = out.unwrap_or_else(|| "oobo-card.svg".to_string());
        std::fs::write(&path, &svg).map_err(|e| format!("cannot write {path}: {e}"))?;
        if is_structured {
            println!("saved: {path}");
        } else {
            eprintln!("\n  saved to {path}");
        }
    } else {
        let png = svg_to_png(&svg)?;
        let path = out.unwrap_or_else(|| "oobo-card.png".to_string());
        std::fs::write(&path, &png).map_err(|e| format!("cannot write {path}: {e}"))?;
        if is_structured {
            println!("saved: {path}");
        } else {
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
    sessions: i64,
}

#[allow(dead_code)]
mod colors {
    pub const PRIMARY: &str = "#ffffff";
    pub const CYAN: &str = "#0ea5e9";
    pub const TEAL: &str = "#14b8a6";
    pub const BG: &str = "#f8fafb";
    pub const DARK: &str = "#111111";
    pub const BORDER: &str = "#e5e7eb";
    pub const SECONDARY: &str = "#4b5563";
    pub const EMPTY_CELL: &str = "#ebedf0";
    pub const MUTED: &str = "#9ca3af";
    pub const MUTED_LIGHT: &str = "#6b7280";
    pub const TEAL_SHADES: [&str; 4] = ["#ccfbf1", "#5eead4", "#14b8a6", "#0d7d72"];
    pub const CYAN_SHADES: [&str; 4] = ["#e0f2fe", "#7dd3fc", "#38bdf8", "#0284c7"];
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

    let mut session_map: HashMap<String, i64> = HashMap::new();
    if let Ok(mut stmt) = db.conn.prepare(
        "SELECT COALESCE(updated_at, created_at) FROM sessions
         WHERE COALESCE(updated_at, created_at) IS NOT NULL
           AND COALESCE(updated_at, created_at) > 0",
    ) {
        if let Ok(rows) = stmt.query_map([], |row| row.get::<_, i64>(0)) {
            for ts in rows.flatten() {
                let secs = crate::utils::to_epoch_secs(ts);
                if let Some(dt) = chrono::DateTime::from_timestamp(secs, 0) {
                    let date = dt.with_timezone(&chrono::Local).date_naive();
                    if date >= start && date <= today {
                        *session_map
                            .entry(date.format("%Y-%m-%d").to_string())
                            .or_insert(0) += 1;
                    }
                }
            }
        }
    }

    let mut cells = Vec::with_capacity(365);
    let mut d = start;
    while d <= today {
        let key = d.format("%Y-%m-%d").to_string();
        let (commits, ai) = day_map.get(&key).copied().unwrap_or((0, 0));
        let sessions = session_map.get(&key).copied().unwrap_or(0);
        cells.push(DayCell {
            date: d,
            commits,
            ai_assisted: ai,
            sessions,
        });
        d += chrono::Duration::days(1);
    }
    cells
}

/// Current streak of consecutive days with AI usage (sessions or AI-assisted
/// commits) going backwards from today.  Idle days (no commits, no sessions)
/// are tolerated up to 2 consecutive days (e.g. weekends), but longer gaps
/// break the streak.
fn compute_ai_streak(heatmap: &[DayCell]) -> i64 {
    let mut streak: i64 = 0;
    let mut idle_run: i64 = 0;
    const MAX_IDLE_GAP: i64 = 2;
    for day in heatmap.iter().rev() {
        let has_ai = day.ai_assisted > 0 || day.sessions > 0;
        if has_ai {
            streak += 1;
            idle_run = 0;
        } else if day.commits > 0 {
            break;
        } else if streak > 0 {
            idle_run += 1;
            if idle_run > MAX_IDLE_GAP {
                break;
            }
        } else {
            break;
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
        r#"<text x="{pad}" y="{y}" font-size="20" font-weight="bold" fill="{DARK}">{}</text>"#,
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
        sx -= (str_width(label) as i32) * 5 + 6;
        svg_body.push_str(&format!(
            r#"<text x="{sx}" y="{}" font-size="12" font-weight="bold" fill="{DARK}" text-anchor="end">{val}</text>"#,
            y
        ));
        sx -= (str_width(val) as i32) * 7 + 16;
    }

    y += 12;
    svg_body.push_str(&format!(
        r#"<line x1="{pad}" y1="{y}" x2="{}" y2="{y}" stroke="{BORDER}" stroke-width="1" opacity="0.8"/>"#,
        w - pad
    ));

    // ── Contribution grid (full year, 52 weeks x 7 days) ────────────────
    y += 18;

    let cell = 11;
    let gap = 3;
    let step = cell + gap;
    let grid_left = pad + 28;

    let max_activity = card
        .heatmap
        .iter()
        .map(|c| c.commits + c.sessions)
        .max()
        .unwrap_or(1)
        .max(1);

    let first_wd = card
        .heatmap
        .first()
        .map(|c| c.date.weekday().num_days_from_sunday())
        .unwrap_or(0);

    // Month labels
    if !card.heatmap.is_empty() {
        let mut last_month = 0u32;
        for c in &card.heatmap {
            let m = c.date.month();
            if m != last_month {
                let day_offset = (c.date - card.heatmap[0].date).num_days() as u32;
                let week_col = (day_offset + first_wd) / 7;
                let x = grid_left as u32 + week_col * step as u32;
                svg_body.push_str(&format!(
                    r#"<text x="{x}" y="{}" font-size="10" fill="{MUTED_LIGHT}">{}</text>"#,
                    y,
                    c.date.format("%b")
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
    let mut gradient_defs = String::new();
    for (i, day) in card.heatmap.iter().enumerate() {
        let offset = i as u32 + first_wd;
        let col = offset / 7;
        let row = offset % 7;
        let cx = grid_left as u32 + col * step as u32;
        let cy = y as u32 + row * step as u32;
        let cf = heatmap_cell_fill(i, day, max_activity);
        if let Some(def) = cf.gradient_def {
            gradient_defs.push_str(&def);
        }
        svg_body.push_str(&format!(
            r#"<rect x="{cx}" y="{cy}" width="{cell}" height="{cell}" rx="3" fill="{}"/>"#,
            cf.fill
        ));
    }
    y += 7 * step + 6;

    // Legend + AI% badge (right side, same line)
    let legend_y = y + 18;

    // AI percentage badge
    let ai_badge_label = card
        .ai_percentage
        .map(|p| format!("{p:.0}% AI"))
        .unwrap_or_else(|| "N/A".to_string());
    svg_body.push_str(&format!(
        r#"<rect x="{pad}" y="{}" width="64" height="20" rx="10" fill="{CYAN}" opacity="0.15"/>"#,
        legend_y - 14
    ));
    svg_body.push_str(&format!(
        r#"<text x="{}" y="{}" font-size="10" font-weight="bold" fill="{CYAN}" text-anchor="middle">{ai_badge_label}</text>"#,
        pad + 32, legend_y
    ));

    // Legend: AI (blue) + Human (teal) squares
    let mut lx = w - pad;
    for (label, color) in [("AI", CYAN), ("Human", TEAL)].iter().rev() {
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

    y = legend_y + 36;

    // ── Tools strip ─────────────────────────────────────────────────────
    svg_body.push_str(&format!(
        r#"<line x1="{pad}" y1="{y}" x2="{}" y2="{y}" stroke="{BORDER}" stroke-width="1" opacity="0.8"/>"#,
        w - pad
    ));
    y += 20;

    let tool_count = card.top_tools.len().min(5);
    if tool_count > 0 {
        let col_w = (w - 2 * pad) / tool_count as i32;
        for (i, t) in card.top_tools.iter().take(tool_count).enumerate() {
            let tx = pad + i as i32 * col_w;
            svg_body.push_str(&format!(
                r#"<text x="{tx}" y="{y}" font-size="12" font-weight="bold" fill="{DARK}">{}</text>"#,
                xml_escape(&t.name)
            ));
            svg_body.push_str(&format!(
                r#"<text x="{tx}" y="{}" font-size="9" fill="{MUTED_LIGHT}">{} · {}</text>"#,
                y + 14,
                t.sessions,
                tui::format_tokens(t.tokens)
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
    let mut svg = String::with_capacity(svg_body.len() + gradient_defs.len() + 1024);
    svg.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" font-family="{font}">"#
    ));
    if !gradient_defs.is_empty() {
        svg.push_str("<defs>");
        svg.push_str(&gradient_defs);
        svg.push_str("</defs>");
    }
    svg.push_str(&format!(
        r#"<rect width="{w}" height="{h}" rx="12" fill="{PRIMARY}"/>"#
    ));
    svg.push_str(&format!(
        r#"<rect width="{w}" height="{h}" rx="12" fill="none" stroke="{BORDER}" stroke-width="1"/>"#
    ));
    svg.push_str(&svg_body);
    svg.push_str("</svg>\n");
    svg
}

struct CellFill {
    fill: String,
    gradient_def: Option<String>,
}

/// Compute the fill for a heatmap cell.  Pure human → solid teal, pure AI →
/// solid blue, mixed → a left-to-right gradient split proportionally.
/// Lightness is driven by activity volume (more = darker).
fn heatmap_cell_fill(idx: usize, cell: &DayCell, max_activity: i64) -> CellFill {
    if cell.commits == 0 && cell.sessions == 0 {
        return CellFill {
            fill: EMPTY_CELL.to_string(),
            gradient_def: None,
        };
    }

    let total = cell.commits + cell.sessions;
    let ai = (cell.ai_assisted + cell.sessions).min(total);
    let ai_pct = ai as f64 / total.max(1) as f64;
    let human_pct = 1.0 - ai_pct;

    let volume = (total as f64 / max_activity.max(1) as f64).min(1.0);
    let intensity = match volume {
        x if x <= 0.25 => 0.35,
        x if x <= 0.50 => 0.55,
        x if x <= 0.75 => 0.78,
        _ => 1.0,
    };

    let bg: (f64, f64, f64) = (235.0, 237.0, 240.0);
    let teal_base: (f64, f64, f64) = (20.0, 184.0, 166.0);
    let blue_base: (f64, f64, f64) = (14.0, 165.0, 233.0);

    let shade = |base: (f64, f64, f64)| -> String {
        let r = (bg.0 + (base.0 - bg.0) * intensity) as u8;
        let g = (bg.1 + (base.1 - bg.1) * intensity) as u8;
        let b = (bg.2 + (base.2 - bg.2) * intensity) as u8;
        format!("#{r:02x}{g:02x}{b:02x}")
    };

    let teal_hex = shade(teal_base);
    let blue_hex = shade(blue_base);

    if ai_pct < 0.05 {
        CellFill {
            fill: teal_hex,
            gradient_def: None,
        }
    } else if human_pct < 0.05 {
        CellFill {
            fill: blue_hex,
            gradient_def: None,
        }
    } else {
        let stop = (human_pct * 100.0).round() as i32;
        let id = format!("hm{idx}");
        let def = format!(
            r#"<linearGradient id="{id}" x1="0" x2="1" y1="0" y2="0">\
               <stop offset="{stop}%" stop-color="{teal_hex}"/>\
               <stop offset="{stop}%" stop-color="{blue_hex}"/>\
               </linearGradient>"#
        );
        CellFill {
            fill: format!("url(#{id})"),
            gradient_def: Some(def),
        }
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

    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or("cannot create pixmap")?;

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
            chrono::DateTime::from_timestamp(ts, 0).map(|dt| {
                dt.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d")
                    .to_string()
            })
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

fn use_color() -> bool {
    use std::io::IsTerminal;
    std::env::var("NO_COLOR").is_err() && std::io::stderr().is_terminal()
}

fn print_terminal(c: &CardData) {
    if !use_color() {
        print_terminal_plain(c);
        return;
    }
    print_terminal_fancy(c);
}

fn print_terminal_plain(c: &CardData) {
    let ai_pct_str = c
        .ai_percentage
        .map(|p| format!("{:.0}%", p))
        .unwrap_or_else(|| "n/a".to_string());

    eprintln!();
    eprintln!("  {}", c.author);
    eprintln!();

    let kv = |label: &str, value: &str| {
        eprintln!("  {:<16} {}", label, value);
    };

    kv("commits", &format_number(c.ai_commits));
    kv("tokens", &tui::format_tokens(c.total_tokens));
    kv("sessions", &format!("{}", c.session_count));
    kv("active days", &format!("{}", c.active_days));
    kv("ai code", &ai_pct_str);
    if c.ai_streak > 0 {
        kv("streak", &format!("{}d", c.ai_streak));
    }

    if !c.top_tools.is_empty() {
        eprintln!();
        for t in &c.top_tools {
            eprintln!(
                "  {:<16} {:>4} sessions   {:>8} tokens",
                t.name,
                t.sessions,
                tui::format_tokens(t.tokens)
            );
        }
    }

    if !c.top_models.is_empty() {
        eprintln!();
        for m in &c.top_models {
            eprintln!("  {:<16} {:>5.1}%", m.name, m.pct);
        }
    }

    eprintln!();
    eprintln!(
        "  oobo v{}                                    oobo.ai",
        env!("CARGO_PKG_VERSION")
    );
    eprintln!();
}

fn print_terminal_fancy(c: &CardData) {
    use chrono::Datelike;

    const W: usize = 72;
    let reset = "\x1b[0m";
    let bold = "\x1b[1m";
    let dim = "\x1b[90m";

    fn fg(r: u8, g: u8, b: u8) -> String {
        format!("\x1b[38;2;{r};{g};{b}m")
    }
    let cyan_fg = fg(14, 165, 233);
    let teal_fg = fg(20, 184, 166);
    let border_fg = fg(75, 85, 99);

    let pad = |s: &str, width: usize| -> String {
        let visible_len: usize = {
            let mut len = 0usize;
            let mut in_esc = false;
            for ch in s.chars() {
                if ch == '\x1b' {
                    in_esc = true;
                } else if in_esc {
                    if ch.is_ascii_alphabetic() {
                        in_esc = false;
                    }
                } else {
                    len += unicode_width(ch);
                }
            }
            len
        };
        if visible_len >= width {
            s.to_string()
        } else {
            format!("{}{}", s, " ".repeat(width - visible_len))
        }
    };

    let line = |content: &str| {
        let padded = pad(content, W);
        eprintln!("  {border_fg}│{reset}  {padded}  {border_fg}│{reset}");
    };
    let separator = || {
        eprintln!("  {border_fg}│  {}│{reset}", "─".repeat(W));
    };
    let blank = || line("");

    // top border
    eprintln!();
    eprintln!("  {border_fg}┌{}┐{reset}", "─".repeat(W + 4));

    blank();

    // header: name left, stats right
    let streak_str = if c.ai_streak > 0 {
        format!(" · {}d 🔥", c.ai_streak)
    } else {
        String::new()
    };
    let stats_right = format!(
        "{} commits · {} tokens",
        format_number(c.ai_commits),
        tui::format_tokens(c.total_tokens),
    );
    let stats_right2 = format!(
        "{} sessions · {} days{}",
        c.session_count, c.active_days, streak_str,
    );

    let name_w = c.author.chars().count();
    let gap1 = W.saturating_sub(name_w + visible_len(&stats_right));
    line(&format!(
        "{bold}{}{reset}{}{}{}{}",
        c.author,
        " ".repeat(gap1),
        dim,
        stats_right,
        reset,
    ));
    let gap2 = W.saturating_sub(visible_len(&stats_right2));
    line(&format!(
        "{}{}{}{}",
        " ".repeat(gap2),
        dim,
        stats_right2,
        reset,
    ));

    separator();
    blank();

    // heatmap
    if !c.heatmap.is_empty() {
        let max_activity = c
            .heatmap
            .iter()
            .map(|d| d.commits + d.sessions)
            .max()
            .unwrap_or(1)
            .max(1);

        let first_wd = c
            .heatmap
            .first()
            .map(|d| d.date.weekday().num_days_from_sunday())
            .unwrap_or(0);

        let total_cols = (c.heatmap.len() as u32 + first_wd).div_ceil(7);

        // month labels — only for the visible portion of the grid
        let col_limit = total_cols.min(W as u32 / 2);
        let col_start = total_cols.saturating_sub(col_limit);
        let mut month_positions: Vec<(usize, String)> = Vec::new();
        let mut last_month = 0u32;
        for d in &c.heatmap {
            let m = d.date.month();
            if m != last_month {
                let day_offset = (d.date - c.heatmap[0].date).num_days() as u32;
                let col = (day_offset + first_wd) / 7;
                if col >= col_start {
                    month_positions
                        .push(((col - col_start) as usize, d.date.format("%b").to_string()));
                }
                last_month = m;
            }
        }
        let visible_cols = col_limit as usize;
        let mut ml = vec![' '; visible_cols * 2];
        for (col, label) in &month_positions {
            let pos = col * 2;
            for (i, ch) in label.chars().enumerate() {
                if pos + i < ml.len() {
                    ml[pos + i] = ch;
                }
            }
        }
        let mut month_label: String = ml.iter().collect();
        if month_label.len() > W {
            month_label.truncate(W);
        }
        line(&format!("{dim}{}{reset}", month_label));

        // grid: 7 rows
        let bg_r = 45u8;
        let bg_g = 45u8;
        let bg_b = 45u8;
        let teal_base: (f64, f64, f64) = (20.0, 184.0, 166.0);
        let blue_base: (f64, f64, f64) = (14.0, 165.0, 233.0);
        let bg_base: (f64, f64, f64) = (bg_r as f64, bg_g as f64, bg_b as f64);
        let empty_fg = fg(bg_r, bg_g, bg_b);

        for row in 0..7u32 {
            let mut row_str = String::new();
            for col in col_start..col_start + col_limit {
                if col > col_start {
                    row_str.push(' ');
                }
                let idx_offset = col * 7 + row;
                if idx_offset < first_wd {
                    row_str.push_str(&format!("{}▄{reset}", empty_fg));
                    continue;
                }
                let idx = (idx_offset - first_wd) as usize;
                if idx >= c.heatmap.len() {
                    row_str.push_str(&format!("{}▄{reset}", empty_fg));
                    continue;
                }
                let day = &c.heatmap[idx];
                if day.commits == 0 && day.sessions == 0 {
                    row_str.push_str(&format!("{}▄{reset}", empty_fg));
                    continue;
                }

                let total = day.commits + day.sessions;
                let ai = (day.ai_assisted + day.sessions).min(total);
                let ai_pct = ai as f64 / total.max(1) as f64;

                let volume = (total as f64 / max_activity.max(1) as f64).min(1.0);
                let intensity = match volume {
                    x if x <= 0.25 => 0.35,
                    x if x <= 0.50 => 0.55,
                    x if x <= 0.75 => 0.78,
                    _ => 1.0,
                };

                let base = if ai_pct >= 0.5 { blue_base } else { teal_base };
                let r = (bg_base.0 + (base.0 - bg_base.0) * intensity) as u8;
                let g = (bg_base.1 + (base.1 - bg_base.1) * intensity) as u8;
                let b = (bg_base.2 + (base.2 - bg_base.2) * intensity) as u8;

                row_str.push_str(&format!("{}▄{reset}", fg(r, g, b)));
            }
            line(&row_str);
        }

        blank();

        // legend
        let ai_pct_legend = c
            .ai_percentage
            .map(|p| format!("{:.0}% AI", p))
            .unwrap_or_else(|| "N/A".to_string());
        line(&format!(
            "{cyan_fg}█ {ai_pct_legend}{reset}       {teal_fg}█ Human{reset}       {dim}█ No activity{reset}",
        ));

        blank();
        separator();
        blank();
    }

    // tools
    if !c.top_tools.is_empty() {
        for t in &c.top_tools {
            let tool_str = format!(
                "{bold}{:<16}{reset} {dim}{:>4} sessions    {:>8} tokens{reset}",
                t.name,
                t.sessions,
                tui::format_tokens(t.tokens),
            );
            line(&tool_str);
        }
        blank();
    }

    // models with bars
    if !c.top_models.is_empty() {
        let name_col = 22usize;
        let bar_max = W.saturating_sub(name_col + 8);
        for m in &c.top_models {
            let filled = ((m.pct / 100.0) * bar_max as f64).round() as usize;
            let empty = bar_max.saturating_sub(filled);
            let bar = format!(
                "{cyan_fg}{}{dim}{}{reset}",
                "█".repeat(filled),
                "░".repeat(empty),
            );
            let model_str = format!(
                "{dim}{:<width$}{reset}{} {:>3.0}%",
                m.name,
                bar,
                m.pct,
                width = name_col,
            );
            line(&model_str);
        }
        blank();
    }

    // footer
    separator();
    let ver = format!("oobo v{}", env!("CARGO_PKG_VERSION"));
    let site = "oobo.ai";
    let footer_gap = W.saturating_sub(ver.len() + site.len());
    line(&format!(
        "{dim}{}{}{}{reset}",
        ver,
        " ".repeat(footer_gap),
        site,
    ));

    // bottom border
    eprintln!("  {border_fg}└{}┘{reset}", "─".repeat(W + 4));
    eprintln!();
}

fn str_width(s: &str) -> usize {
    s.chars().map(unicode_width).sum()
}

fn unicode_width(ch: char) -> usize {
    if ('\u{1100}'..='\u{115F}').contains(&ch)
        || ('\u{2E80}'..='\u{A4CF}').contains(&ch)
        || ('\u{AC00}'..='\u{D7A3}').contains(&ch)
        || ('\u{F900}'..='\u{FAFF}').contains(&ch)
        || ('\u{FE10}'..='\u{FE6F}').contains(&ch)
        || ('\u{FF01}'..='\u{FF60}').contains(&ch)
        || ('\u{FFE0}'..='\u{FFE6}').contains(&ch)
        || ('\u{1F000}'..='\u{1FAFF}').contains(&ch)
    {
        2
    } else {
        1
    }
}

fn visible_len(s: &str) -> usize {
    let mut len = 0usize;
    let mut in_esc = false;
    for ch in s.chars() {
        if ch == '\x1b' {
            in_esc = true;
        } else if in_esc {
            if ch.is_ascii_alphabetic() {
                in_esc = false;
            }
        } else {
            len += unicode_width(ch);
        }
    }
    len
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
        md.push_str(&format!("| Commits | {} |\n", c.ai_commits));
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

fn print_agent_compact(c: &CardData) {
    println!("author: {}", crate::utils::sanitize_pipe(&c.author));
    println!("tools: {}", c.tool_count);
    println!("projects: {}", c.project_count);
    println!("sessions: {}", c.session_count);
    println!("tokens: {}", tui::format_tokens(c.total_tokens));
    if let Some(ai_pct) = c.ai_percentage {
        println!("ai_code: {:.1}%", ai_pct);
    }
    println!("ai_commits: {}", c.ai_commits);
    if let Some(cpd) = c.commits_per_day {
        println!("commits_per_day: {:.1}", cpd);
    }
    println!("active_days: {}", c.active_days);
    if let Some(ref since) = c.active_since {
        println!("active_since: {since}");
    }
    if !c.top_tools.is_empty() {
        println!("# tool | sessions | tokens");
        for t in &c.top_tools {
            println!(
                "{} | {} | {}",
                t.name,
                t.sessions,
                tui::format_tokens(t.tokens),
            );
        }
    }
    if !c.top_models.is_empty() {
        println!("# model | sessions | pct");
        for m in &c.top_models {
            println!("{} | {} | {:.0}%", m.name, m.sessions, m.pct);
        }
    }
}

fn print_json(c: &CardData, include_svg: bool) {
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

    let mut json = serde_json::json!({
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

    if include_svg {
        let svg = render_svg(c);
        json["svg"] = serde_json::Value::String(svg);
    }

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
    fn test_heatmap_cell_empty() {
        let cell = DayCell {
            date: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            commits: 0,
            ai_assisted: 0,
            sessions: 0,
        };
        let cf = heatmap_cell_fill(0, &cell, 10);
        assert_eq!(cf.fill, EMPTY_CELL);
        assert!(cf.gradient_def.is_none());
    }

    #[test]
    fn test_heatmap_cell_pure_human() {
        let cell = DayCell {
            date: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            commits: 5,
            ai_assisted: 0,
            sessions: 0,
        };
        let cf = heatmap_cell_fill(0, &cell, 10);
        assert!(cf.fill.starts_with('#'));
        assert!(cf.gradient_def.is_none());
    }

    #[test]
    fn test_heatmap_cell_pure_ai() {
        let cell = DayCell {
            date: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            commits: 5,
            ai_assisted: 5,
            sessions: 0,
        };
        let cf = heatmap_cell_fill(0, &cell, 10);
        assert!(cf.fill.starts_with('#'));
        assert!(cf.gradient_def.is_none());
    }

    #[test]
    fn test_heatmap_cell_session_only() {
        let cell = DayCell {
            date: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            commits: 0,
            ai_assisted: 0,
            sessions: 3,
        };
        let cf = heatmap_cell_fill(0, &cell, 10);
        assert!(cf.fill.starts_with('#'));
        assert!(cf.gradient_def.is_none());
    }

    #[test]
    fn test_heatmap_cell_mixed_has_gradient() {
        let cell = DayCell {
            date: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            commits: 10,
            ai_assisted: 4,
            sessions: 0,
        };
        let cf = heatmap_cell_fill(42, &cell, 10);
        assert!(cf.fill.starts_with("url(#hm42)"));
        assert!(cf.gradient_def.is_some());
        let def = cf.gradient_def.unwrap();
        assert!(def.contains("hm42"));
        assert!(def.contains("60%")); // 6 human / 10 total = 60% human stop
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
        assert!(svg.contains("45% AI"));
        assert!(svg.contains("500.0K"));
        assert!(svg.contains("Cursor"));
        assert!(svg.ends_with("</svg>\n"));
    }

    #[test]
    fn test_render_svg_no_ai_data_shows_na() {
        let card = CardData {
            author: "TestDev".to_string(),
            tool_count: 1,
            project_count: 1,
            session_count: 5,
            total_tokens: 1000,
            ai_percentage: None,
            ai_commits: 0,
            commits_per_day: None,
            active_days: 1,
            active_since: None,
            ai_streak: 0,
            top_tools: vec![],
            top_models: vec![],
            weekly_trend: vec![],
            heatmap: vec![],
        };

        let svg = render_svg(&card);
        assert!(
            svg.contains("N/A"),
            "SVG should show N/A when ai_percentage is None"
        );
        assert!(
            !svg.contains("0% AI"),
            "SVG should not show 0% AI when there's no data"
        );
    }

    #[test]
    fn test_streak_breaks_on_idle_before_ai_activity() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 3, 27).unwrap();

        // No human-only firewall between old AI and idle tail.
        // Old buggy code: idle → fall through → idle → fall through → AI → streak=1
        // Fixed code: idle → break → streak=0
        let heatmap = vec![
            DayCell {
                date: today - chrono::Duration::days(2),
                commits: 0,
                ai_assisted: 1,
                sessions: 0,
            },
            DayCell {
                date: today - chrono::Duration::days(1),
                commits: 0,
                ai_assisted: 0,
                sessions: 0,
            },
            DayCell {
                date: today,
                commits: 0,
                ai_assisted: 0,
                sessions: 0,
            },
        ];

        let streak = compute_ai_streak(&heatmap);
        assert_eq!(
            streak, 0,
            "idle days at the end should not count old disconnected AI activity"
        );
    }

    #[test]
    fn test_streak_counts_consecutive_ai_days() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 3, 27).unwrap();

        let heatmap = vec![
            DayCell {
                date: today - chrono::Duration::days(3),
                commits: 0,
                ai_assisted: 0,
                sessions: 0,
            },
            DayCell {
                date: today - chrono::Duration::days(2),
                commits: 0,
                ai_assisted: 1,
                sessions: 0,
            },
            DayCell {
                date: today - chrono::Duration::days(1),
                commits: 0,
                ai_assisted: 0,
                sessions: 2,
            },
            DayCell {
                date: today,
                commits: 0,
                ai_assisted: 1,
                sessions: 1,
            },
        ];

        let streak = compute_ai_streak(&heatmap);
        assert_eq!(
            streak, 3,
            "three consecutive AI days should give streak of 3"
        );
    }

    #[test]
    fn test_streak_skips_gaps_within_active_streak() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 3, 27).unwrap();

        let heatmap = vec![
            DayCell {
                date: today - chrono::Duration::days(3),
                commits: 0,
                ai_assisted: 0,
                sessions: 1,
            },
            DayCell {
                date: today - chrono::Duration::days(2),
                commits: 0,
                ai_assisted: 0,
                sessions: 0,
            },
            DayCell {
                date: today - chrono::Duration::days(1),
                commits: 0,
                ai_assisted: 0,
                sessions: 0,
            },
            DayCell {
                date: today,
                commits: 0,
                ai_assisted: 0,
                sessions: 1,
            },
        ];

        let streak = compute_ai_streak(&heatmap);
        assert_eq!(streak, 2, "idle gaps within a streak should be skipped");
    }

    #[test]
    fn test_streak_human_only_day_breaks_streak() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 3, 27).unwrap();

        // today: AI, day -1: human-only (commits but no AI), day -2: AI
        // The human-only day should stop the streak at 1.
        let heatmap = vec![
            DayCell {
                date: today - chrono::Duration::days(2),
                commits: 0,
                ai_assisted: 0,
                sessions: 1,
            },
            DayCell {
                date: today - chrono::Duration::days(1),
                commits: 5,
                ai_assisted: 0,
                sessions: 0,
            },
            DayCell {
                date: today,
                commits: 0,
                ai_assisted: 0,
                sessions: 1,
            },
        ];

        let streak = compute_ai_streak(&heatmap);
        assert_eq!(
            streak, 1,
            "human-only commit day should break the AI streak"
        );
    }

    #[test]
    fn test_streak_breaks_on_long_idle_gap() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 3, 27).unwrap();

        let heatmap = vec![
            DayCell {
                date: today - chrono::Duration::days(4),
                commits: 0,
                ai_assisted: 0,
                sessions: 1,
            },
            DayCell {
                date: today - chrono::Duration::days(3),
                commits: 0,
                ai_assisted: 0,
                sessions: 0,
            },
            DayCell {
                date: today - chrono::Duration::days(2),
                commits: 0,
                ai_assisted: 0,
                sessions: 0,
            },
            DayCell {
                date: today - chrono::Duration::days(1),
                commits: 0,
                ai_assisted: 0,
                sessions: 0,
            },
            DayCell {
                date: today,
                commits: 0,
                ai_assisted: 0,
                sessions: 1,
            },
        ];

        let streak = compute_ai_streak(&heatmap);
        assert_eq!(streak, 1, "3 consecutive idle days should break the streak");
    }

    #[test]
    fn test_render_markdown_commit_label() {
        let card = CardData {
            author: "TestDev".to_string(),
            tool_count: 1,
            project_count: 1,
            session_count: 10,
            total_tokens: 1000,
            ai_percentage: Some(30.0),
            ai_commits: 50,
            commits_per_day: Some(2.0),
            active_days: 10,
            active_since: None,
            ai_streak: 0,
            top_tools: vec![],
            top_models: vec![],
            weekly_trend: vec![],
            heatmap: vec![],
        };

        let md = render_markdown(&card);
        assert!(
            md.contains("| Commits | 50 |"),
            "should use 'Commits' not 'AI-Assisted Commits'"
        );
        assert!(
            !md.contains("AI-Assisted Commits"),
            "old mislabel should not appear"
        );
    }

    #[test]
    fn test_svg_month_labels_use_first_wd_offset() {
        // Jan 1 2026 is Thursday (first_wd=4). The Feb label should be placed
        // using (day_offset + first_wd) / 7, not day_offset / 7.
        // Feb 1 is day_offset=31.
        // Old (buggy): week_col = 31 / 7 = 4 → x = 60 + 4*14 = 116
        // New (fixed): week_col = (31 + 4) / 7 = 5 → x = 60 + 5*14 = 130
        let start = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let mut heatmap = Vec::new();
        for i in 0..60 {
            heatmap.push(DayCell {
                date: start + chrono::Duration::days(i),
                commits: 0,
                ai_assisted: 0,
                sessions: 0,
            });
        }

        let first_wd = start.weekday().num_days_from_sunday();
        assert_eq!(
            first_wd, 4,
            "Jan 1 2026 should be Thursday (4 days from Sunday)"
        );

        let card = CardData {
            author: "Test".to_string(),
            tool_count: 0,
            project_count: 0,
            session_count: 0,
            total_tokens: 0,
            ai_percentage: None,
            ai_commits: 0,
            commits_per_day: None,
            active_days: 0,
            active_since: None,
            ai_streak: 0,
            top_tools: vec![],
            top_models: vec![],
            weekly_trend: vec![],
            heatmap,
        };

        let svg = render_svg(&card);

        let grid_left: u32 = 32 + 28; // pad + 28
        let step: u32 = 14; // cell(11) + gap(3)
        let feb_day_offset: u32 = 31;
        let correct_col = (feb_day_offset + first_wd) / 7; // 5
        let buggy_col = feb_day_offset / 7; // 4
        assert_ne!(
            correct_col, buggy_col,
            "formulas should differ for this date"
        );

        let correct_x = grid_left + correct_col * step;
        let buggy_x = grid_left + buggy_col * step;

        let correct_tag = format!("x=\"{}\"", correct_x);
        let buggy_tag = format!("x=\"{}\"", buggy_x);

        // Find the Feb label and verify its x-coordinate
        assert!(svg.contains("Feb"), "SVG should contain Feb label");
        let feb_pos = svg.find("Feb").unwrap();
        let label_region = &svg[feb_pos.saturating_sub(80)..feb_pos];
        assert!(label_region.contains(&correct_tag),
            "Feb label should be at x={correct_x} (col {correct_col}), not x={buggy_x} (col {buggy_col})");
        assert!(
            !label_region.contains(&buggy_tag),
            "Feb label should not be at the old buggy position x={buggy_x}"
        );
    }
}
