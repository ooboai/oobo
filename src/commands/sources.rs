use crate::cli::OutputMode;
use crate::db::Db;

pub fn run_cmd(mode: OutputMode) -> Result<(), String> {
    match mode {
        OutputMode::Agent => run_agent(),
        OutputMode::Json => run_json(),
        OutputMode::Tui => run(),
    }
}

fn run_agent() -> Result<(), String> {
    let db = Db::open()?;
    let tools = gather_tool_sources(&db)?;

    println!("# tool | sessions | tokens | source");
    for t in &tools {
        let tok = if t.total_tokens > 0 {
            let suffix = if t.has_native_tokens {
                " (native)"
            } else {
                " (est.)"
            };
            format!("{}{}", crate::tui::format_tokens(t.total_tokens), suffix)
        } else {
            "0".to_string()
        };
        println!(
            "{} | {} | {} | {}",
            t.label, t.session_count, tok, t.source_desc,
        );
    }
    Ok(())
}

fn run_json() -> Result<(), String> {
    let db = Db::open()?;
    let tools = gather_tool_sources(&db)?;
    let api_keys = gather_api_keys();

    let tools_json: Vec<serde_json::Value> = tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "label": t.label,
                "session_count": t.session_count,
                "total_tokens": t.total_tokens,
                "has_native_tokens": t.has_native_tokens,
                "source_desc": t.source_desc,
            })
        })
        .collect();

    let keys_json: Vec<serde_json::Value> = api_keys
        .iter()
        .map(|(name, configured)| {
            serde_json::json!({
                "name": name,
                "configured": configured,
            })
        })
        .collect();

    let json = serde_json::json!({
        "tools": tools_json,
        "api_keys": keys_json,
    });
    crate::utils::print_json(&json);
    Ok(())
}

pub fn run() -> Result<(), String> {
    let db = Db::open()?;

    eprintln!("oobo data sources\n");

    let tools = gather_tool_sources(&db)?;
    let cursor_tracking = gather_cursor_tracking();
    let api_keys = gather_api_keys();

    eprintln!("{}", "─".repeat(70));
    eprintln!(
        "{:<14} {:>8} {:>18}  Data Source",
        "Tool", "Sessions", "Tokens"
    );
    eprintln!("{}", "─".repeat(70));

    for t in &tools {
        let sessions_str = if t.session_count > 0 {
            t.session_count.to_string()
        } else {
            "—".to_string()
        };
        let tokens_str = if t.has_native_tokens {
            format!("{} (native)", crate::tui::format_tokens(t.total_tokens))
        } else if t.total_tokens > 0 {
            format!("{} (est.)", crate::tui::format_tokens(t.total_tokens))
        } else {
            "—".to_string()
        };
        eprintln!(
            "{:<14} {:>8} {:>18}  {}",
            t.label, sessions_str, tokens_str, t.source_desc
        );
    }

    eprintln!("{}", "─".repeat(70));

    if let Some(ct) = &cursor_tracking {
        eprintln!();
        eprintln!("Cursor AI Code Tracking:");
        eprintln!(
            "  {} scored commits, {:.1}% AI-written code",
            ct.commits, ct.ai_pct
        );
        eprintln!("  Source: {}", ct.db_path);
    }

    if let Some(ds) = gather_cursor_daily_stats() {
        eprintln!();
        eprintln!("Cursor Daily Stats:");
        eprintln!(
            "  {} days tracked, {} total prompts",
            ds.days, ds.prompt_count
        );
        eprintln!(
            "  Composer: {} suggested / {} accepted lines",
            ds.composer_suggested, ds.composer_accepted
        );
        eprintln!(
            "  Tab: {} suggested / {} accepted lines",
            ds.tab_suggested, ds.tab_accepted
        );
    }

    if let Some((input, output, cost, events)) = crate::tools::aider::analytics::global_stats() {
        eprintln!();
        eprintln!("Aider Analytics Log:");
        eprintln!(
            "  {} events, {}+{} tokens, ${:.4} cost",
            events,
            crate::tui::format_tokens(input as i64),
            crate::tui::format_tokens(output as i64),
            cost,
        );
        eprintln!(
            "  Source: {}",
            crate::tools::aider::analytics::analytics_log_path().display()
        );
    } else if !crate::tools::aider::analytics::is_aider_config_set() {
        let has_aider = tools
            .iter()
            .any(|t| t.label == "Aider" && t.session_count > 0);
        if has_aider {
            eprintln!();
            eprintln!("Aider Analytics: not configured");
            eprintln!(
                "  Add to ~/.aider.conf.yml: {}",
                crate::tools::aider::analytics::config_snippet()
            );
        }
    }

    if let Some((input, output, cache_read, cache_create, prompts)) =
        crate::tools::zed::telemetry::global_stats()
    {
        eprintln!();
        eprintln!("Zed Telemetry Log:");
        let total = input + output + cache_read + cache_create;
        eprintln!(
            "  {} prompts, {} tokens ({}in + {}out + {}cache)",
            prompts,
            crate::tui::format_tokens(total as i64),
            crate::tui::format_tokens(input as i64),
            crate::tui::format_tokens(output as i64),
            crate::tui::format_tokens((cache_read + cache_create) as i64),
        );
        if let Some(path) = crate::tools::zed::telemetry::telemetry_log_path() {
            eprintln!("  Source: {}", path.display());
        }
    }

    if let Ok(api_totals) = db.api_usage_totals() {
        if !api_totals.is_empty() {
            eprintln!();
            eprintln!("Remote API Usage (last 30 days):");
            for (source, summary) in &api_totals {
                if summary.has_data() {
                    let label = match source.as_str() {
                        "anthropic" => "Anthropic",
                        "openai" => "OpenAI",
                        _ => source.as_str(),
                    };
                    let tokens = summary.total_tokens() as i64;
                    let tok_str = crate::tui::format_tokens(tokens);
                    eprintln!("  {label}: {tok_str} tokens ({} days)", summary.days);
                }
            }
        }
    }

    eprintln!();
    eprintln!("API keys:");
    for (name, status) in &api_keys {
        let icon = if *status { "✓" } else { "✗" };
        eprintln!("  {icon} {name}");
    }

    eprintln!();
    eprintln!("Data coverage:");

    let native_count: usize = tools.iter().filter(|t| t.has_native_tokens).count();
    let estimated_count: usize = tools
        .iter()
        .filter(|t| !t.has_native_tokens && t.session_count > 0)
        .count();

    if native_count > 0 {
        let names: Vec<&str> = tools
            .iter()
            .filter(|t| t.has_native_tokens)
            .map(|t| t.label)
            .collect();
        eprintln!("  Native tokens/cost: {}", names.join(", "));
    }
    if estimated_count > 0 {
        let names: Vec<&str> = tools
            .iter()
            .filter(|t| !t.has_native_tokens && t.session_count > 0)
            .map(|t| t.label)
            .collect();
        eprintln!("  Estimated (tiktoken): {}", names.join(", "));
    }
    if cursor_tracking.is_some() {
        eprintln!("  AI code attribution: Cursor (per-commit, per-branch)");
    }

    Ok(())
}

struct ToolSource {
    label: &'static str,
    session_count: i64,
    total_tokens: i64,
    has_native_tokens: bool,
    source_desc: &'static str,
}

struct CursorTracking {
    commits: i64,
    ai_pct: f64,
    db_path: String,
}

fn gather_tool_sources(db: &Db) -> Result<Vec<ToolSource>, String> {
    let per_tool = db.aggregate_stats_per_tool()?;
    let mut sources = Vec::new();

    let known_tools: &[(&str, &str, bool, &str)] = &[
        ("claude", "Claude Code", true, "local JSONL (native tokens)"),
        (
            "codex",
            "Codex CLI",
            true,
            "local JSONL + SQLite (native tokens)",
        ),
        ("opencode", "OpenCode", true, "local SQLite (native tokens)"),
        (
            "copilot",
            "Copilot Chat",
            false,
            "VS Code chat sessions (tiktoken est.)",
        ),
        ("composer", "Cursor", false, "composerData + ai-tracking.db"),
        (
            "windsurf",
            "Windsurf",
            false,
            "cascade .pb (encrypted, metadata only)",
        ),
        ("aider", "Aider", false, "local sessions (tiktoken est.)"),
        (
            "trae",
            "Trae",
            false,
            "SQLCipher DB (encrypted, metadata only)",
        ),
        ("zed", "Zed", false, "local sessions (tiktoken est.)"),
        ("gemini", "Gemini CLI", true, "local JSON (native tokens)"),
    ];

    let aider_has_analytics = crate::tools::aider::analytics::has_analytics_log();
    let zed_has_telemetry = crate::tools::zed::telemetry::has_telemetry_log();

    for (key, label, native_tokens, desc) in known_tools {
        let stats = per_tool.iter().find(|(s, _)| s == key);
        let (session_count, total_tokens) = match stats {
            Some((_, agg)) => (
                agg.session_count,
                agg.total_input_tokens + agg.total_output_tokens,
            ),
            None => (0, 0),
        };

        let (nt, d) = if *key == "aider" && aider_has_analytics {
            (true, "analytics JSONL (native tokens)")
        } else if *key == "zed" && zed_has_telemetry {
            (true, "telemetry.log (native tokens)")
        } else {
            (*native_tokens, *desc)
        };

        sources.push(ToolSource {
            label,
            session_count,
            total_tokens,
            has_native_tokens: nt && session_count > 0,
            source_desc: d,
        });
    }

    sources.sort_by(|a, b| b.session_count.cmp(&a.session_count));
    Ok(sources)
}

fn gather_cursor_tracking() -> Option<CursorTracking> {
    let home = dirs::home_dir()?;
    let db_path = home
        .join(".cursor")
        .join("ai-tracking")
        .join("ai-code-tracking.db");
    if !db_path.exists() {
        return None;
    }

    let conn = crate::utils::open_db_readonly(&db_path).ok()?;

    let commits: i64 = conn
        .query_row("SELECT COUNT(*) FROM scored_commits", [], |r| r.get(0))
        .ok()?;

    if commits == 0 {
        return None;
    }

    let ai_pct: f64 = conn
        .query_row(
            "SELECT COALESCE(100.0 * SUM(composerLinesAdded) / NULLIF(SUM(linesAdded), 0), 0.0) FROM scored_commits",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0.0);

    Some(CursorTracking {
        commits,
        ai_pct,
        db_path: db_path.display().to_string(),
    })
}

struct CursorDailyStats {
    days: usize,
    prompt_count: i64,
    composer_suggested: i64,
    composer_accepted: i64,
    tab_suggested: i64,
    tab_accepted: i64,
}

fn gather_cursor_daily_stats() -> Option<CursorDailyStats> {
    let db_path = crate::tools::cursor::state_vscdb_path()?;
    if !db_path.exists() {
        return None;
    }

    let conn = crate::utils::open_db_readonly(&db_path).ok()?;

    let prompt_count: i64 = conn
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM ItemTable WHERE key = 'freeBestOfN.promptCount'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let mut stmt = conn
        .prepare("SELECT value FROM ItemTable WHERE key LIKE 'aiCodeTracking.dailyStats%'")
        .ok()?;

    let rows = stmt
        .query_map([], |row| {
            let v: String = row.get(0)?;
            Ok(v)
        })
        .ok()?;

    let mut days = 0;
    let mut composer_suggested: i64 = 0;
    let mut composer_accepted: i64 = 0;
    let mut tab_suggested: i64 = 0;
    let mut tab_accepted: i64 = 0;

    for row in rows.flatten() {
        let v: serde_json::Value = match serde_json::from_str(&row) {
            Ok(v) => v,
            Err(_) => continue,
        };
        days += 1;
        composer_suggested += v
            .get("composerSuggestedLines")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        composer_accepted += v
            .get("composerAcceptedLines")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        tab_suggested += v
            .get("tabSuggestedLines")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        tab_accepted += v
            .get("tabAcceptedLines")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
    }

    if days == 0 {
        return None;
    }

    Some(CursorDailyStats {
        days,
        prompt_count,
        composer_suggested,
        composer_accepted,
        tab_suggested,
        tab_accepted,
    })
}

fn gather_api_keys() -> Vec<(&'static str, bool)> {
    let cfg = crate::config::Config::load_or_default();
    vec![
        ("Anthropic Admin API", !cfg.claude.api_key.is_empty()),
        ("GitHub Copilot PAT", !cfg.copilot.api_key.is_empty()),
        ("Windsurf service key", !cfg.windsurf.api_key.is_empty()),
        ("OpenAI API key", !cfg.codex.api_key.is_empty()),
        ("Google AI Studio key", !cfg.gemini.api_key.is_empty()),
    ]
}
