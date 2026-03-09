use crate::commands::index;
use crate::config::Config;
use crate::db::Db;
use crate::scanner;

pub fn run(cfg: &Config, project: Option<String>, quiet: bool) -> Result<(), String> {
    let db = Db::open()?;

    let result = if let Some(ref path) = project {
        let resolved = crate::paths::normalize_path(path);
        if !quiet {
            eprintln!("scanning project: {resolved}");
        }
        scanner::scan_project(&db, cfg, &resolved)?
    } else {
        if !quiet {
            eprintln!("scanning all AI tools...");
        }
        scanner::full_scan(&db, cfg)?
    };

    if !quiet {
        eprintln!(
            "found {} project(s), {} session(s)",
            result.projects_found, result.sessions_found
        );
    }

    let sessions = if let Some(ref path) = project {
        let resolved = crate::paths::normalize_path(path);
        if let Ok(pid) = index::find_project_id(&db, &resolved) {
            db.list_unindexed_sessions_by_project(&pid)
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    } else {
        db.list_unindexed_sessions().unwrap_or_default()
    };

    if !sessions.is_empty() {
        if !quiet {
            eprintln!("indexing {} new session(s)...", sessions.len());
        }
        let idx = index::index_sessions(&db, &sessions, false, !quiet);
        if !quiet && idx.indexed > 0 {
            eprintln!("indexed {} session(s)", idx.indexed);
        }
    } else if !quiet {
        eprintln!("all sessions already indexed");
    }

    if !quiet {
        check_aider_analytics_hint(&sessions);
    }

    Ok(())
}

fn check_aider_analytics_hint(sessions: &[crate::db::sessions::SessionRow]) {
    let has_aider = sessions.iter().any(|s| s.source == "aider");
    if !has_aider {
        return;
    }

    if crate::tools::aider::analytics::has_analytics_log() {
        return;
    }

    if crate::tools::aider::analytics::is_aider_config_set() {
        return;
    }

    eprintln!();
    eprintln!("hint: Aider sessions found but analytics log is not configured.");
    eprintln!("      Add this to ~/.aider.conf.yml for native token/cost tracking:");
    eprintln!();
    eprintln!(
        "        {}",
        crate::tools::aider::analytics::config_snippet()
    );
    eprintln!();
    eprintln!("      This upgrades Aider from estimated to native telemetry.");
}
