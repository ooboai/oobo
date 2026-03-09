use std::collections::HashSet;

use crate::config::Config;
use crate::db::projects::ProjectRow;
use crate::db::sessions::SessionRow;
use crate::db::Db;
use crate::paths;
use crate::tools::cursor::Session;

/// Run a full scan: discover all projects and sessions from every enabled tool.
pub fn full_scan(db: &Db, cfg: &Config) -> Result<ScanResult, String> {
    let mut result = ScanResult::default();

    let all_sessions = collect_all_sessions(cfg);

    let mut project_tools: std::collections::HashMap<String, HashSet<String>> =
        std::collections::HashMap::new();

    for session in &all_sessions {
        project_tools
            .entry(session.project_path.clone())
            .or_default()
            .insert(session.source.clone());
    }

    let now = chrono::Utc::now().timestamp();

    for (project_path, tools) in &project_tools {
        if project_path.is_empty() {
            continue;
        }
        let slug = paths::slug_from_path(project_path);
        let name = std::path::Path::new(project_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&slug)
            .to_string();

        let git_remote = detect_git_remote(project_path);

        let mut tools_vec: Vec<String> = tools.iter().cloned().collect();
        tools_vec.sort();

        let project = ProjectRow {
            id: slug.clone(),
            path: project_path.clone(),
            name,
            git_remote,
            discovered_at: now,
            last_seen_at: now,
            last_scanned_at: now,
            tools: tools_vec,
        };

        db.upsert_project(&project)?;
        result.projects_found += 1;

        let project_dir = paths::oobo_project_dir(project_path);
        let _ = paths::ensure_dir(&project_dir);
    }

    for session in &all_sessions {
        if session.project_path.is_empty() {
            continue;
        }
        let project_id = paths::slug_from_path(&session.project_path);

        let session_row = SessionRow {
            id: session.session_id.clone(),
            source: session.source.clone(),
            project_id,
            name: if session.name.is_empty() {
                None
            } else {
                Some(session.name.clone())
            },
            mode: if session.mode.is_empty() {
                None
            } else {
                Some(session.mode.clone())
            },
            model: None,
            created_at: session.created_at,
            updated_at: session.updated_at,
            message_count: 0,
            first_message: None,
            indexed_at: now,
        };

        db.upsert_session(&session_row)?;
        result.sessions_found += 1;
    }

    Ok(result)
}

/// Scan only a specific project by path.
pub fn scan_project(db: &Db, cfg: &Config, project_path: &str) -> Result<ScanResult, String> {
    let mut result = ScanResult::default();
    let mut tools = HashSet::new();

    let sessions = collect_sessions_for_project(cfg, project_path);
    let now = chrono::Utc::now().timestamp();

    for session in &sessions {
        tools.insert(session.source.clone());
    }

    let slug = paths::slug_from_path(project_path);
    let name = std::path::Path::new(project_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&slug)
        .to_string();

    let git_remote = detect_git_remote(project_path);
    let mut tools_vec: Vec<String> = tools.into_iter().collect();
    tools_vec.sort();

    let project = ProjectRow {
        id: slug.clone(),
        path: project_path.to_string(),
        name,
        git_remote,
        discovered_at: now,
        last_seen_at: now,
        last_scanned_at: now,
        tools: tools_vec,
    };

    db.upsert_project(&project)?;
    result.projects_found = 1;

    let project_dir = paths::oobo_project_dir(project_path);
    let _ = paths::ensure_dir(&project_dir);

    for session in &sessions {
        let session_row = SessionRow {
            id: session.session_id.clone(),
            source: session.source.clone(),
            project_id: slug.clone(),
            name: if session.name.is_empty() {
                None
            } else {
                Some(session.name.clone())
            },
            mode: if session.mode.is_empty() {
                None
            } else {
                Some(session.mode.clone())
            },
            model: None,
            created_at: session.created_at,
            updated_at: session.updated_at,
            message_count: 0,
            first_message: None,
            indexed_at: now,
        };

        db.upsert_session(&session_row)?;
        result.sessions_found += 1;
    }

    db.update_project_last_scanned(&slug, now)?;
    Ok(result)
}

/// Quick scan: only re-scan projects that haven't been scanned recently.
#[allow(dead_code)]
pub fn quick_scan(db: &Db, cfg: &Config, max_age_secs: i64) -> Result<ScanResult, String> {
    let now = chrono::Utc::now().timestamp();
    let projects = db.list_projects()?;

    let mut total = ScanResult::default();

    for project in &projects {
        if now - project.last_scanned_at < max_age_secs {
            continue;
        }
        let sub = scan_project(db, cfg, &project.path)?;
        total.projects_found += sub.projects_found;
        total.sessions_found += sub.sessions_found;
    }

    let existing_paths: HashSet<String> = projects.iter().map(|p| p.path.clone()).collect();
    let all_sessions = collect_all_sessions(cfg);
    let new_paths: HashSet<String> = all_sessions
        .iter()
        .map(|s| s.project_path.clone())
        .filter(|p| !p.is_empty() && !existing_paths.contains(p))
        .collect();

    for path in &new_paths {
        let sub = scan_project(db, cfg, path)?;
        total.projects_found += sub.projects_found;
        total.sessions_found += sub.sessions_found;
    }

    Ok(total)
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub projects_found: usize,
    pub sessions_found: usize,
}

fn collect_all_sessions(cfg: &Config) -> Vec<Session> {
    crate::session::all_sessions(cfg)
}

fn collect_sessions_for_project(cfg: &Config, project_root: &str) -> Vec<Session> {
    crate::session::all_for_project(project_root, cfg)
}

fn detect_git_remote(project_path: &str) -> Option<String> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let output = std::process::Command::new(git)
        .args(["remote", "get-url", "origin"])
        .current_dir(project_path)
        .output()
        .ok()?;

    if output.status.success() {
        let remote = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !remote.is_empty() {
            return Some(remote);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_result_default() {
        let r = ScanResult::default();
        assert_eq!(r.projects_found, 0);
        assert_eq!(r.sessions_found, 0);
    }

    #[test]
    fn test_full_scan_empty_config() {
        let db = Db::open_in_memory().unwrap();
        let cfg = Config {
            cursor: crate::config::ToolConfig {
                enabled: false,
                api_key: String::new(),
            },
            claude: crate::config::ToolConfig {
                enabled: false,
                api_key: String::new(),
            },
            windsurf: crate::config::ToolConfig {
                enabled: false,
                api_key: String::new(),
            },
            trae: crate::config::ToolConfig {
                enabled: false,
                api_key: String::new(),
            },
            aider: crate::config::ToolConfig {
                enabled: false,
                api_key: String::new(),
            },
            copilot: crate::config::ToolConfig {
                enabled: false,
                api_key: String::new(),
            },
            zed: crate::config::ToolConfig {
                enabled: false,
                api_key: String::new(),
            },
            codex: crate::config::ToolConfig {
                enabled: false,
                api_key: String::new(),
            },
            opencode: crate::config::ToolConfig {
                enabled: false,
                api_key: String::new(),
            },
            gemini: crate::config::ToolConfig {
                enabled: false,
                api_key: String::new(),
            },
            ..Default::default()
        };
        let result = full_scan(&db, &cfg).unwrap();
        assert_eq!(result.projects_found, 0);
        assert_eq!(result.sessions_found, 0);
    }
}
