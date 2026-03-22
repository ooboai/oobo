use std::fs;
use std::path::PathBuf;

use crate::tools::cursor::Session;
use crate::tools::vscode_fork;

/// Windsurf stores Cascade conversations in encrypted .pb files under
/// ~/.codeium/windsurf/cascade/{uuid}.pb. We can discover sessions from
/// these files and get metadata from workspace storage, but cannot read
/// the actual conversation content (ChaCha20-Poly1305 encrypted).
fn codeium_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codeium").join("windsurf"))
}

fn cascade_dir() -> Option<PathBuf> {
    codeium_dir().map(|d| d.join("cascade"))
}

/// Discover sessions from encrypted .pb files in the cascade directory.
fn sessions_from_cascade() -> Vec<CascadeSession> {
    let dir = match cascade_dir() {
        Some(d) if d.is_dir() => d,
        _ => return Vec::new(),
    };

    let mut sessions = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("pb") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    let mtime = path
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as i64);

                    sessions.push(CascadeSession {
                        session_id: stem.to_string(),
                        updated_at: mtime,
                    });
                }
            }
        }
    }
    sessions
}

struct CascadeSession {
    session_id: String,
    updated_at: Option<i64>,
}

/// Try to resolve a session's workspace/project from Windsurf workspace storage.
fn resolve_workspace_for_session(session_id: &str) -> Option<String> {
    let ws_dirs = vscode_fork::find_all_workspace_dirs("Windsurf").ok()?;

    for (ws_dir, folder_path) in &ws_dirs {
        let db_path = ws_dir.join("state.vscdb");
        if !db_path.exists() {
            continue;
        }

        let conn = match crate::utils::open_db_readonly(&db_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let mut stmt = match conn.prepare(
            "SELECT value FROM ItemTable WHERE key LIKE '%cascade%' OR key LIKE '%session%'",
        ) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let rows = match stmt.query_map([], |row| {
            let v: String = row.get(0)?;
            Ok(v)
        }) {
            Ok(r) => r,
            Err(_) => continue,
        };

        for row in rows.flatten() {
            if row.contains(session_id) {
                return Some(folder_path.clone());
            }
        }
    }

    // Fallback: if only one workspace, associate the session with it
    let ws_dirs = vscode_fork::find_all_workspace_dirs("Windsurf").ok()?;
    if ws_dirs.len() == 1 {
        return Some(ws_dirs[0].1.clone());
    }

    None
}

fn build_sessions(cascade_sessions: Vec<CascadeSession>) -> Vec<Session> {
    cascade_sessions
        .into_iter()
        .map(|cs| {
            let project_path = resolve_workspace_for_session(&cs.session_id).unwrap_or_default();

            Session {
                session_id: cs.session_id,
                name: "Cascade session".to_string(),
                mode: "cascade".to_string(),
                created_at: cs.updated_at,
                updated_at: cs.updated_at,
                project_path,
                workspace_dir: String::new(),
                source: "windsurf".to_string(),
                parent_session_id: None,
                subagent_type: None,
            }
        })
        .collect()
}

pub fn sessions_for_project(project_root: &str) -> Result<Vec<Session>, String> {
    let norm = crate::paths::normalize_path(project_root);
    let cascade = sessions_from_cascade();
    let mut sessions = build_sessions(cascade);
    sessions.retain(|s| {
        !s.project_path.is_empty() && crate::paths::normalize_path(&s.project_path) == norm
    });
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

pub fn all_sessions() -> Result<Vec<Session>, String> {
    let cascade = sessions_from_cascade();
    let mut sessions = build_sessions(cascade);
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

pub mod transcript {
    use std::path::PathBuf;

    use crate::tools::cursor::transcript::Message;

    pub fn find_transcript_path(_project_path: &str, _session_id: &str) -> Option<PathBuf> {
        // Windsurf conversations are encrypted — no transcript available
        None
    }

    pub fn count_messages(_project_path: &str, _session_id: &str) -> u32 {
        0
    }

    pub fn parse_messages(_path: &std::path::Path) -> Vec<Message> {
        Vec::new()
    }

    pub fn read_transcript(_path: &std::path::Path, _max_messages: u32) -> String {
        String::from("(Windsurf Cascade conversations are encrypted and cannot be read directly)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cascade_dir_exists() {
        // Just verify the function doesn't panic
        let _ = cascade_dir();
    }

    #[test]
    fn test_sessions_from_cascade_empty() {
        // If cascade dir doesn't exist, should return empty
        let sessions = sessions_from_cascade();
        // May or may not be empty depending on environment
        let _ = sessions;
    }
}
