/// Factory Droid — Claude Code-compatible hooks, JSONL sessions in ~/.factory/.
///
/// Config hierarchy:
///   ~/.factory/settings.json (user-level)
///   .factory/settings.json (project-level)
///
/// Droid stores sessions similar to Claude Code: per-project JSONL files in a
/// projects directory keyed by the slugified CWD. Also checks project-level
/// .factory/ directories for local session data.
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::tools::cursor::Session;

pub fn droid_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".factory"))
}

pub fn sessions_dir() -> Option<PathBuf> {
    droid_dir().map(|d| d.join("sessions"))
}

/// Droid (like Claude) may also store per-project data in ~/.factory/projects/
fn projects_dir() -> Option<PathBuf> {
    droid_dir().map(|d| d.join("projects"))
}

fn session_from_jsonl(path: &Path, project_path_fallback: &str) -> Option<Session> {
    let session_id = path.file_stem().and_then(|s| s.to_str())?.to_string();

    let file = fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);

    let mut name: Option<String> = None;
    let mut project_path = String::new();
    let mut created_at: Option<i64> = None;
    let mut updated_at: Option<i64> = None;

    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let entry: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if name.is_none() {
            if let Some(title) = entry
                .get("summary")
                .or_else(|| entry.get("title"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                name = Some(crate::utils::truncate_name(
                    title,
                    crate::utils::MAX_SESSION_NAME_LEN,
                ));
            }
        }

        if project_path.is_empty() {
            if let Some(cwd) = entry.get("cwd").and_then(|v| v.as_str()) {
                project_path = cwd.to_string();
            }
        }

        if let Some(ts) = entry.get("timestamp").and_then(serde_json::Value::as_i64) {
            if created_at.is_none() {
                created_at = Some(ts);
            }
            updated_at = Some(ts);
        }
    }

    // Only fall back to the caller-supplied path if the file itself had no cwd
    if project_path.is_empty() && !project_path_fallback.is_empty() {
        project_path = project_path_fallback.to_string();
    }

    let mtime = fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64
        });

    Some(Session {
        session_id,
        name: name.unwrap_or_else(|| "Droid session".to_string()),
        mode: "droid".to_string(),
        created_at: created_at.or(mtime),
        updated_at: updated_at.or(mtime),
        project_path,
        workspace_dir: path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        source: "droid".to_string(),
        parent_session_id: None,
        subagent_type: None,
    })
}

fn collect_jsonl_sessions(dir: &Path, project_path: &str, sessions: &mut Vec<Session>) {
    if !dir.exists() {
        return;
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() && p.extension().is_some_and(|e| e == "jsonl") {
                if let Some(s) = session_from_jsonl(&p, project_path) {
                    sessions.push(s);
                }
            }
        }
    }
}

/// Scan ~/.factory/projects/<slug>/ directories (Claude-style layout).
fn collect_from_projects_dir(sessions: &mut Vec<Session>) {
    let dir = match projects_dir() {
        Some(d) if d.exists() => d,
        _ => return,
    };

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let project_dir = entry.path();
            if !project_dir.is_dir() {
                continue;
            }
            let project_path = slug_to_path(&project_dir);
            collect_jsonl_sessions(&project_dir, &project_path, sessions);
        }
    }
}

/// Best-effort reverse of Claude-style path slugification.
/// e.g. `-Users-dev-myapp` -> `/Users/dev/myapp`
///
/// WARNING: This is lossy — original hyphens in path segments are
/// indistinguishable from separator hyphens. The cwd field from within
/// JSONL files always takes priority over this guess (see session_from_jsonl).
fn slug_to_path(dir: &Path) -> String {
    dir.file_name()
        .and_then(|n| n.to_str())
        .map(|slug| slug.replacen('-', "/", 1).replace('-', "/"))
        .unwrap_or_default()
}

/// Scan a project-level .factory/ directory for JSONL session files.
fn collect_from_project_level(project_root: &str, sessions: &mut Vec<Session>) {
    let project_factory = Path::new(project_root).join(".factory");
    if project_factory.is_dir() {
        collect_jsonl_sessions(&project_factory, project_root, sessions);
        let sub_sessions = project_factory.join("sessions");
        collect_jsonl_sessions(&sub_sessions, project_root, sessions);
    }
}

pub fn sessions_for_project(project_root: &str) -> Result<Vec<Session>, String> {
    let norm_root = crate::paths::normalize_path(project_root);
    let mut sessions = Vec::new();

    // User-level ~/.factory/sessions/
    if let Some(dir) = sessions_dir() {
        collect_jsonl_sessions(&dir, "", &mut sessions);
    }

    // User-level ~/.factory/projects/<slug>/
    collect_from_projects_dir(&mut sessions);

    // Project-level .factory/
    collect_from_project_level(project_root, &mut sessions);

    sessions.retain(|s| {
        !s.project_path.is_empty() && crate::paths::normalize_path(&s.project_path) == norm_root
    });
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    sessions.dedup_by(|a, b| a.session_id == b.session_id);
    Ok(sessions)
}

pub fn all_sessions() -> Result<Vec<Session>, String> {
    let mut sessions = Vec::new();

    if let Some(dir) = sessions_dir() {
        collect_jsonl_sessions(&dir, "", &mut sessions);
    }
    collect_from_projects_dir(&mut sessions);

    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    sessions.dedup_by(|a, b| a.session_id == b.session_id);
    Ok(sessions)
}

pub mod transcript {
    use std::fs;
    use std::io::BufRead;
    use std::path::{Path, PathBuf};

    use crate::core::message::Message;

    pub fn find_transcript_path(project_path: &str, session_id: &str) -> Option<PathBuf> {
        let filename = format!("{session_id}.jsonl");

        // Check ~/.factory/sessions/
        if let Some(dir) = super::sessions_dir() {
            let p = dir.join(&filename);
            if p.exists() {
                return Some(p);
            }
        }

        // Check ~/.factory/projects/<slug>/
        if let Some(dir) = super::projects_dir() {
            if dir.exists() {
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        let candidate = entry.path().join(&filename);
                        if candidate.exists() {
                            return Some(candidate);
                        }
                    }
                }
            }
        }

        // Check project-level .factory/
        if !project_path.is_empty() {
            let project_factory = Path::new(project_path).join(".factory");
            let p = project_factory.join(&filename);
            if p.exists() {
                return Some(p);
            }
            let p = project_factory.join("sessions").join(&filename);
            if p.exists() {
                return Some(p);
            }
        }

        None
    }

    pub fn parse_messages(path: &Path) -> Vec<Message> {
        let file = match fs::File::open(path) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };
        let reader = std::io::BufReader::new(file);
        let mut messages = Vec::new();

        for line in reader.lines().map_while(Result::ok) {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            let entry: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let role = entry.get("role").and_then(|v| v.as_str()).unwrap_or("");
            if role != "user" && role != "assistant" {
                continue;
            }
            let text = entry
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !text.is_empty() {
                messages.push(Message {
                    role: role.to_string(),
                    text,
                    timestamp_ms: entry.get("timestamp").and_then(serde_json::Value::as_i64),
                });
            }
        }
        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_from_jsonl() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sess-abc.jsonl");
        fs::write(
            &path,
            r#"{"cwd":"/dev/myapp","summary":"Refactor auth module"}
{"role":"user","content":"Refactor the auth module","timestamp":1700000000000}
{"role":"assistant","content":"Done.","timestamp":1700000010000}
"#,
        )
        .unwrap();

        let session = session_from_jsonl(&path, "").unwrap();
        assert_eq!(session.session_id, "sess-abc");
        assert_eq!(session.name, "Refactor auth module");
        assert_eq!(session.project_path, "/dev/myapp");
        assert_eq!(session.source, "droid");
    }

    #[test]
    fn test_session_from_jsonl_with_override() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sess-xyz.jsonl");
        fs::write(
            &path,
            r#"{"role":"user","content":"Hello","timestamp":1700000000000}
"#,
        )
        .unwrap();

        // Fallback path used because JSONL has no cwd field
        let session = session_from_jsonl(&path, "/override/path").unwrap();
        assert_eq!(session.project_path, "/override/path");
    }

    #[test]
    fn test_session_from_jsonl_cwd_overrides_fallback() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sess-pri.jsonl");
        fs::write(
            &path,
            r#"{"cwd":"/real/project/path","role":"user","content":"Hello","timestamp":1700000000000}
"#,
        )
        .unwrap();

        // cwd from JSONL takes priority over fallback
        let session = session_from_jsonl(&path, "/wrong/fallback").unwrap();
        assert_eq!(session.project_path, "/real/project/path");
    }

    #[test]
    fn test_collect_from_project_level() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project = tmp.path().join("myproject");
        let factory = project.join(".factory");
        fs::create_dir_all(&factory).unwrap();
        fs::write(
            factory.join("session-1.jsonl"),
            r#"{"role":"user","content":"Hello","timestamp":1000}
"#,
        )
        .unwrap();

        let mut sessions = Vec::new();
        collect_from_project_level(project.to_str().unwrap(), &mut sessions);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].project_path, project.to_str().unwrap());
    }

    #[test]
    fn test_slug_to_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("-Users-dev-myapp");
        assert_eq!(slug_to_path(&dir), "/Users/dev/myapp");
    }

    #[test]
    fn test_transcript_parse_messages() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sess.jsonl");
        fs::write(
            &path,
            r#"{"role":"user","content":"Fix bug","timestamp":1000}
{"role":"tool","content":"result"}
{"role":"assistant","content":"Fixed.","timestamp":2000}
"#,
        )
        .unwrap();

        let msgs = transcript::parse_messages(&path);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].text, "Fix bug");
        assert_eq!(msgs[1].text, "Fixed.");
    }

    #[test]
    fn test_find_transcript_in_project_level_factory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project = tmp.path().join("myproject");
        let factory = project.join(".factory");
        fs::create_dir_all(&factory).unwrap();
        let session_file = factory.join("sess-proj.jsonl");
        fs::write(
            &session_file,
            r#"{"role":"user","content":"Hello","timestamp":1000}
"#,
        )
        .unwrap();

        let found = transcript::find_transcript_path(project.to_str().unwrap(), "sess-proj");
        assert_eq!(found, Some(session_file));
    }
}
