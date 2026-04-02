use crate::cli::OutputMode;
use crate::config::Config;
use crate::db::Db;

pub fn run(fix: bool, mode: OutputMode) -> Result<(), String> {
    let mut checks: Vec<CheckResult> = vec![
        check_config(),
        check_db(),
        check_git_alias(),
        check_git_hooks(),
        check_agent_hooks(),
        check_stale_sessions(),
        check_tools(),
    ];

    if fix {
        for check in &mut checks {
            if (check.status == Status::Fail || check.status == Status::Warn)
                && check.fix_fn.is_some()
            {
                let fix_fn = check.fix_fn.take().unwrap();
                match fix_fn() {
                    Ok(msg) => {
                        check.status = Status::Fixed;
                        check.detail = msg;
                    }
                    Err(e) => {
                        check.detail = format!("fix failed: {e}");
                    }
                }
            }
        }
    }

    if mode.is_structured() {
        if mode == OutputMode::Agent {
            println!("# name | status | detail");
            for c in &checks {
                let status = match c.status {
                    Status::Ok => "ok",
                    Status::Warn => "warn",
                    Status::Fail => "fail",
                    Status::Fixed => "fixed",
                };
                let detail = crate::utils::sanitize_pipe(&c.detail);
                println!("{} | {} | {}", c.name, status, detail);
            }
        } else {
            let items: Vec<serde_json::Value> = checks
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "name": c.name,
                        "status": match c.status {
                            Status::Ok => "ok",
                            Status::Warn => "warn",
                            Status::Fail => "fail",
                            Status::Fixed => "fixed",
                        },
                        "detail": c.detail,
                    })
                })
                .collect();
            crate::utils::print_json(&serde_json::json!({ "checks": items }));
        }
        return Ok(());
    }

    let pass = checks
        .iter()
        .filter(|c| c.status == Status::Ok || c.status == Status::Fixed)
        .count();
    let warn = checks.iter().filter(|c| c.status == Status::Warn).count();
    let fail = checks.iter().filter(|c| c.status == Status::Fail).count();

    for check in &checks {
        let icon = match check.status {
            Status::Ok => "\x1b[32m✓\x1b[0m",
            Status::Warn => "\x1b[33m!\x1b[0m",
            Status::Fail => "\x1b[31m✗\x1b[0m",
            Status::Fixed => "\x1b[36m↻\x1b[0m",
        };
        println!("  {icon} {:<24} {}", check.name, check.detail);
    }

    println!();
    if fail > 0 {
        println!("  {pass} passed, {warn} warnings, {fail} failures");
        if !fix {
            println!("  run \x1b[1moobo inspect --fix\x1b[0m to auto-repair");
        }
    } else if warn > 0 {
        println!("  {pass} passed, {warn} warnings");
    } else {
        println!("  all {pass} checks passed");
    }

    Ok(())
}

#[derive(PartialEq)]
enum Status {
    Ok,
    Warn,
    Fail,
    Fixed,
}

struct CheckResult {
    name: String,
    status: Status,
    detail: String,
    fix_fn: Option<Box<dyn FnOnce() -> Result<String, String>>>,
}

fn check_config() -> CheckResult {
    let path = Config::config_path();
    if path.exists() {
        let cfg = Config::load_or_default();
        let reg = crate::tools::registry();
        let tools: Vec<&str> = reg
            .all()
            .filter(|t| t.enabled(&cfg))
            .map(|t| t.name())
            .collect();

        CheckResult {
            name: "config".into(),
            status: Status::Ok,
            detail: format!("{} tools enabled ({})", tools.len(), tools.join(", ")),
            fix_fn: None,
        }
    } else {
        CheckResult {
            name: "config".into(),
            status: Status::Warn,
            detail: "no config file — run oobo setup".into(),
            fix_fn: None,
        }
    }
}

fn check_db() -> CheckResult {
    match Db::open() {
        Ok(db) => {
            let projects = db.list_projects().map(|p| p.len()).unwrap_or(0);
            let sessions: i64 = db
                .conn
                .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
                .unwrap_or(0);
            let anchors: i64 = db
                .conn
                .query_row("SELECT COUNT(*) FROM anchors", [], |r| r.get(0))
                .unwrap_or(0);
            CheckResult {
                name: "database".into(),
                status: Status::Ok,
                detail: format!("{projects} projects, {sessions} sessions, {anchors} anchors"),
                fix_fn: None,
            }
        }
        Err(e) => CheckResult {
            name: "database".into(),
            status: Status::Fail,
            detail: format!("cannot open: {e}"),
            fix_fn: None,
        },
    }
}

fn check_git_alias() -> CheckResult {
    let shell = std::env::var("SHELL").unwrap_or_default();
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => {
            return CheckResult {
                name: "git alias".into(),
                status: Status::Warn,
                detail: "cannot detect home dir".into(),
                fix_fn: None,
            }
        }
    };

    let rc_files: Vec<std::path::PathBuf> = [".zshrc", ".bashrc", ".bash_profile"]
        .iter()
        .map(|f| home.join(f))
        .filter(|p| p.exists())
        .collect();

    let fish_config = home.join(".config/fish/config.fish");
    let all_files: Vec<std::path::PathBuf> = rc_files
        .into_iter()
        .chain(if fish_config.exists() || shell.contains("fish") {
            vec![fish_config]
        } else {
            vec![]
        })
        .collect();

    for rc in &all_files {
        if let Ok(content) = std::fs::read_to_string(rc) {
            if content.contains("alias git=oobo") || content.contains("alias git oobo") {
                return CheckResult {
                    name: "git alias".into(),
                    status: Status::Ok,
                    detail: format!(
                        "installed in {}",
                        rc.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    fix_fn: None,
                };
            }
        }
    }

    CheckResult {
        name: "git alias".into(),
        status: Status::Warn,
        detail: "not installed — run oobo alias install".into(),
        fix_fn: Some(Box::new(|| {
            crate::alias::install_alias()?;
            Ok("installed".into())
        })),
    }
}

fn check_git_hooks() -> CheckResult {
    let cfg = Config::load_or_default();
    let root = match crate::git::proxy::project_root(&cfg) {
        Some(r) => r,
        None => {
            return CheckResult {
                name: "git hooks".into(),
                status: Status::Warn,
                detail: "not in a git repository".into(),
                fix_fn: None,
            }
        }
    };

    let hooks_dir = crate::git::detect::resolve_git_dir(&root).join("hooks");
    let post_commit = hooks_dir.join("post-commit");

    if post_commit.exists() {
        if let Ok(content) = std::fs::read_to_string(&post_commit) {
            if content.contains("oobo") {
                return CheckResult {
                    name: "git hooks".into(),
                    status: Status::Ok,
                    detail: "post-commit hook installed".into(),
                    fix_fn: None,
                };
            }
        }
    }

    let root_clone = root.clone();
    CheckResult {
        name: "git hooks".into(),
        status: Status::Fail,
        detail: "post-commit hook missing".into(),
        fix_fn: Some(Box::new(move || {
            crate::hooks::install::install_project_hooks(&root_clone)?;
            Ok("hooks installed".into())
        })),
    }
}

fn check_agent_hooks() -> CheckResult {
    let installed = crate::hooks::install::check_installed_hooks();
    if installed.is_empty() {
        CheckResult {
            name: "agent hooks".into(),
            status: Status::Fail,
            detail: "no agent hooks found".into(),
            fix_fn: Some(Box::new(|| {
                let results = crate::hooks::install::install_all_agent_hooks();
                if results.is_empty() {
                    Err("no hooks could be installed".into())
                } else {
                    Ok(format!("{} tool(s): {}", results.len(), results.join(", ")))
                }
            })),
        }
    } else {
        CheckResult {
            name: "agent hooks".into(),
            status: Status::Ok,
            detail: format!("{} tool(s): {}", installed.len(), installed.join(", ")),
            fix_fn: None,
        }
    }
}

fn check_stale_sessions() -> CheckResult {
    let cfg = Config::load_or_default();
    let root = match crate::git::proxy::project_root(&cfg) {
        Some(r) => r,
        None => {
            return CheckResult {
                name: "session state".into(),
                status: Status::Ok,
                detail: "not in a git repo — skipped".into(),
                fix_fn: None,
            }
        }
    };

    let all = crate::hooks::state::active_sessions(&root);
    if all.is_empty() {
        return CheckResult {
            name: "session state".into(),
            status: Status::Ok,
            detail: "no active sessions".into(),
            fix_fn: None,
        };
    }

    let now = chrono::Utc::now().timestamp();
    let stale_threshold = 86400i64;
    let stale: Vec<&crate::hooks::state::ActiveSession> = all
        .iter()
        .filter(|s| now - s.updated_at > stale_threshold)
        .collect();

    if !stale.is_empty() {
        let stale_count = stale.len();
        CheckResult {
            name: "session state".into(),
            status: Status::Warn,
            detail: format!("{stale_count} stale session file(s) (>24h old) — harmless, data preserved for indexing"),
            fix_fn: None,
        }
    } else {
        let with_wt = all.iter().filter(|s| s.worktree.is_some()).count();
        let detail = if with_wt > 0 && with_wt < all.len() {
            format!(
                "{} active session(s), {} across worktrees",
                all.len(),
                with_wt
            )
        } else {
            format!("{} active session(s)", all.len())
        };
        CheckResult {
            name: "session state".into(),
            status: Status::Ok,
            detail,
            fix_fn: None,
        }
    }
}

fn check_tools() -> CheckResult {
    let cfg = Config::load_or_default();
    let registry = crate::tools::registry();
    let enabled: Vec<&str> = registry.enabled(&cfg).map(|t| t.name()).collect();

    let mut reachable = Vec::new();
    for tool in &enabled {
        let has_sessions = registry
            .by_name(tool)
            .and_then(|t| t.all_sessions().ok())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        if has_sessions {
            reachable.push(*tool);
        }
    }

    if reachable.is_empty() && !enabled.is_empty() {
        CheckResult {
            name: "tools".into(),
            status: Status::Warn,
            detail: format!(
                "{} enabled but no sessions found — try oobo scan",
                enabled.len()
            ),
            fix_fn: None,
        }
    } else {
        CheckResult {
            name: "tools".into(),
            status: Status::Ok,
            detail: format!("{} with data: {}", reachable.len(), reachable.join(", ")),
            fix_fn: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_config_returns_result() {
        let result = check_config();
        assert!(!result.name.is_empty());
        assert!(!result.detail.is_empty());
    }

    #[test]
    fn test_check_db_works() {
        let result = check_db();
        assert_eq!(result.name, "database");
    }

    #[test]
    fn test_status_equality() {
        assert!(Status::Ok == Status::Ok);
        assert!(Status::Fail != Status::Ok);
    }
}
