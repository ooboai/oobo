//! Bare `oobo` (no subcommand) — four-quadrant behavior:
//!
//! - Inside a repo: delegates to `anchors` (with limit=50) for agent/json,
//!   or an anchor-feed TUI for pretty (Phase 3.8 will introduce a proper
//!   unified TUI; for v1 we fall back to the anchors list).
//! - Outside a repo: shows a cross-project listing.

use crate::cli::OutputMode;
use crate::config::Config;
use crate::db::Db;

const BARE_LIMIT: usize = 50;

pub fn run(cfg: &Config, mode: OutputMode) -> Result<i32, String> {
    match crate::git::proxy::project_root(cfg) {
        Some(_) => in_repo(cfg, mode),
        None => cross_project(cfg, mode),
    }
}

fn in_repo(cfg: &Config, mode: OutputMode) -> Result<i32, String> {
    match mode {
        OutputMode::Tui => crate::tui::app::run(cfg),
        _ => {
            // Byte-for-byte equivalence with `oobo anchors --<mode> --limit 50`.
            crate::commands::anchors::run(cfg, BARE_LIMIT, mode)?;
            Ok(0)
        }
    }
}

fn cross_project(cfg: &Config, mode: OutputMode) -> Result<i32, String> {
    let db = match Db::open() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("oobo: warning: {e}");
            emit_empty(mode);
            return Ok(0);
        }
    };
    let projects = db.list_projects().unwrap_or_default();
    if projects.is_empty() {
        emit_empty(mode);
        return Ok(0);
    }

    let mut rows: Vec<ProjectStats> = Vec::with_capacity(projects.len());
    for p in &projects {
        let settings = db.get_project_settings(&p.id).unwrap_or_default();
        let enabled = !settings.ignored;
        let stats = db.anchor_stats_for_project(&p.id).unwrap_or_default();
        rows.push(ProjectStats {
            id: p.id.clone(),
            name: p.name.clone(),
            path: p.path.clone(),
            remote: p.git_remote.clone(),
            enabled,
            last_activity: stats.last_activity,
            anchors: stats.anchors,
            tokens: stats.tokens,
            ai_pct: stats.ai_pct,
        });
    }
    rows.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));

    match mode {
        OutputMode::Json => emit_json(&rows, cfg),
        OutputMode::Agent => emit_agent(&rows),
        OutputMode::Tui => emit_pretty(&rows),
    }
    Ok(0)
}

struct ProjectStats {
    id: String,
    name: String,
    path: String,
    remote: Option<String>,
    enabled: bool,
    last_activity: i64,
    anchors: i64,
    tokens: i64,
    ai_pct: i64,
}

fn emit_empty(mode: OutputMode) {
    match mode {
        OutputMode::Json => {
            let json = serde_json::json!({
                "projects": [],
                "stats": { "projects": 0, "anchors": 0, "tokens": 0, "ai_pct": 0 },
            });
            crate::utils::print_json(&json);
        }
        OutputMode::Agent => {
            println!("no projects tracked. run: oobo setup");
        }
        OutputMode::Tui => {
            println!("oobo: no projects tracked yet. run:");
            println!();
            println!("    oobo setup");
            println!();
            println!("to discover projects and AI sessions on this machine.");
        }
    }
}

fn emit_agent(rows: &[ProjectStats]) {
    for r in rows {
        let rel = relative_time(r.last_activity);
        let flag = if r.enabled { "on" } else { "off" };
        println!(
            "{:<14} {:<5} {} {} {}% {}",
            r.name,
            rel,
            r.anchors,
            human_tokens(r.tokens),
            r.ai_pct,
            flag
        );
    }
}

fn emit_pretty(rows: &[ProjectStats]) {
    let totals = summary(rows);
    println!(
        "oobo · {} projects · {} anchors · {} tok · {}% AI",
        rows.len(),
        totals.anchors,
        human_tokens(totals.tokens),
        totals.ai_pct
    );
    println!();
    for r in rows {
        let rel = relative_time(r.last_activity);
        let disabled = if r.enabled { "" } else { " (disabled)" };
        println!(
            "  {:<16} {:<5} {} anchors · {} · {}% AI{}",
            r.name,
            rel,
            r.anchors,
            human_tokens(r.tokens),
            r.ai_pct,
            disabled
        );
    }
}

fn emit_json(rows: &[ProjectStats], _cfg: &Config) {
    let arr: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "name": r.name,
                "path": r.path,
                "remote": r.remote,
                "enabled": r.enabled,
                "last_activity": r.last_activity,
                "stats": { "anchors": r.anchors, "tokens": r.tokens, "ai_pct": r.ai_pct },
            })
        })
        .collect();
    let totals = summary(rows);
    let json = serde_json::json!({
        "projects": arr,
        "stats": {
            "projects": rows.len(),
            "anchors": totals.anchors,
            "tokens": totals.tokens,
            "ai_pct": totals.ai_pct,
        }
    });
    crate::utils::print_json(&json);
}

struct Summary {
    anchors: i64,
    tokens: i64,
    ai_pct: i64,
}

fn summary(rows: &[ProjectStats]) -> Summary {
    let anchors: i64 = rows.iter().map(|r| r.anchors).sum();
    let tokens: i64 = rows.iter().map(|r| r.tokens).sum();
    let weighted: i64 = rows.iter().map(|r| r.anchors * r.ai_pct).sum();
    let ai_pct = if anchors == 0 { 0 } else { weighted / anchors };
    Summary {
        anchors,
        tokens,
        ai_pct,
    }
}

fn human_tokens(tokens: i64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1000 {
        format!("{}k", tokens / 1000)
    } else {
        tokens.to_string()
    }
}

fn relative_time(ts: i64) -> String {
    if ts <= 0 {
        return "-".to_string();
    }
    let now = chrono::Utc::now().timestamp();
    let d = (now - ts).max(0);
    if d < 60 {
        format!("{d}s")
    } else if d < 3600 {
        format!("{}m", d / 60)
    } else if d < 86400 {
        format!("{}h", d / 3600)
    } else if d < 30 * 86400 {
        format!("{}d", d / 86400)
    } else {
        format!("{}mo", d / (30 * 86400))
    }
}
