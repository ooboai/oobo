use crate::tools::cursor::Session;
use crate::tools::vscode_fork;

/// Trae stores chat data in an encrypted SQLCipher database at
/// ~/Library/Application Support/Trae/ModularData/ai-agent/database.db.
/// We can discover sessions from the workspace state.vscdb metadata
/// (memento/icube-ai-agent-storage) and extract user input text from
/// icube-ai-agent-storage-input-history, but full conversations are
/// not readable without the encryption key.
///
/// Extract sessions from Trae workspace storage by reading the
/// icube-ai-agent-storage memento key which contains session IDs.
fn sessions_from_workspaces() -> Vec<Session> {
    let ws_dirs = match vscode_fork::find_all_workspace_dirs("Trae") {
        Ok(dirs) => dirs,
        Err(_) => return Vec::new(),
    };

    let mut sessions = Vec::new();

    for (ws_dir, folder_path) in &ws_dirs {
        let db_path = ws_dir.join("state.vscdb");
        if !db_path.exists() {
            continue;
        }

        let conn = match crate::utils::open_db_readonly(&db_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let agent_storage: Option<String> = conn
            .query_row(
                "SELECT value FROM ItemTable WHERE key = 'memento/icube-ai-agent-storage'",
                [],
                |row| row.get(0),
            )
            .ok();

        let input_history: Option<String> = conn
            .query_row(
                "SELECT value FROM ItemTable WHERE key = 'icube-ai-agent-storage-input-history'",
                [],
                |row| row.get(0),
            )
            .ok();

        let first_input = input_history
            .as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|v| v.as_array().cloned())
            .and_then(|arr| {
                arr.first()
                    .and_then(|item| item.get("inputText"))
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string())
            });

        if let Some(storage) = agent_storage {
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&storage) {
                if let Some(list) = data.get("list").and_then(|v| v.as_array()) {
                    for (i, entry) in list.iter().enumerate() {
                        let session_id = match entry.get("sessionId").and_then(|v| v.as_str()) {
                            Some(id) if !id.is_empty() => id.to_string(),
                            _ => continue,
                        };

                        let name = if i == 0 { first_input.clone() } else { None };

                        let name = name
                            .map(|s| {
                                crate::utils::truncate_name(&s, crate::utils::MAX_SESSION_NAME_LEN)
                            })
                            .unwrap_or_else(|| "Trae session".to_string());

                        let ts = trae_db_mtime();

                        sessions.push(Session {
                            session_id,
                            name,
                            mode: "agent".to_string(),
                            created_at: ts,
                            updated_at: ts,
                            project_path: folder_path.clone(),
                            workspace_dir: ws_dir.to_string_lossy().to_string(),
                            source: "trae".to_string(),
                            parent_session_id: None,
                            subagent_type: None,
                        });
                    }
                }
            }
        }
    }

    sessions
}

/// Get the modification time of Trae's ai-agent database as a timestamp proxy.
fn trae_db_mtime() -> Option<i64> {
    let db_path = trae_db_path()?;
    db_path
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}

fn trae_db_path() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let p = dirs::home_dir()?
            .join("Library/Application Support/Trae/ModularData/ai-agent/database.db");
        if p.exists() {
            return Some(p);
        }
    }
    #[cfg(target_os = "linux")]
    {
        let p = dirs::config_dir()?.join("Trae/ModularData/ai-agent/database.db");
        if p.exists() {
            return Some(p);
        }
    }
    #[cfg(target_os = "windows")]
    {
        let p = dirs::data_dir()?.join("Trae/ModularData/ai-agent/database.db");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

pub fn sessions_for_project(project_root: &str) -> Result<Vec<Session>, String> {
    let norm = crate::paths::normalize_path(project_root);
    let mut sessions = sessions_from_workspaces();
    sessions.retain(|s| {
        !s.project_path.is_empty() && crate::paths::normalize_path(&s.project_path) == norm
    });
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

pub fn all_sessions() -> Result<Vec<Session>, String> {
    let mut sessions = sessions_from_workspaces();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    Ok(sessions)
}

pub mod transcript {
    use std::path::PathBuf;

    use crate::tools::cursor::transcript::Message;

    pub fn find_transcript_path(_project_path: &str, _session_id: &str) -> Option<PathBuf> {
        // Trae conversations are in an encrypted SQLCipher database
        None
    }

    pub fn count_messages(_project_path: &str, _session_id: &str) -> u32 {
        0
    }

    pub fn parse_messages(_path: &std::path::Path) -> Vec<Message> {
        Vec::new()
    }

    pub fn read_transcript(_path: &std::path::Path, _max_messages: u32) -> String {
        String::from(
            "(Trae conversations are stored in an encrypted database and cannot be read directly)",
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_parse_agent_storage() {
        let json = r#"{"list":[{"isCurrent":true,"sessionId":"69a93d73f7a767db95a1fa7e","messages":[]}],"currentSessionId":"69a93d73f7a767db95a1fa7e"}"#;
        let data: serde_json::Value = serde_json::from_str(json).unwrap();
        let list = data.get("list").unwrap().as_array().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(
            list[0].get("sessionId").unwrap().as_str().unwrap(),
            "69a93d73f7a767db95a1fa7e"
        );
    }

    #[test]
    fn test_parse_input_history() {
        let json = r#"[{"inputText":"can you tell me what this project is all about ?","parsedQuery":["can you tell me what this project is all about ?"],"multiMedia":[]}]"#;
        let data: serde_json::Value = serde_json::from_str(json).unwrap();
        let arr = data.as_array().unwrap();
        assert_eq!(
            arr[0].get("inputText").unwrap().as_str().unwrap(),
            "can you tell me what this project is all about ?"
        );
    }
}
