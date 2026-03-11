use std::io::IsTerminal;

/// Who triggered the current git operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitAuthor {
    /// AI autonomously committed (non-interactive + active session).
    Agent {
        tool: String,
        session_id: Option<String>,
    },
    /// Human committed while collaborating with AI (interactive + active session).
    Assisted {
        tool: String,
        session_id: Option<String>,
    },
    /// Human committed with no AI involvement.
    Human,
    /// CI/CD or script (non-interactive, no AI session).
    Automated,
}

/// Determine whether the current commit was made by an agent, a human,
/// or a human assisted by an agent.
///
/// Detection matrix:
/// | Active Session | Interactive | Result    |
/// |----------------|-------------|-----------|
/// | Yes            | Yes         | Assisted  |
/// | Yes            | No          | Agent     |
/// | No             | Yes         | Human     |
/// | No             | No          | Automated |
pub fn detect(project_root: &str) -> CommitAuthor {
    let interactive = is_interactive();

    if let Some(author) = check_active_sessions(project_root) {
        if interactive {
            return CommitAuthor::Assisted {
                tool: match &author {
                    CommitAuthor::Agent { tool, .. } => tool.clone(),
                    _ => "unknown".into(),
                },
                session_id: match &author {
                    CommitAuthor::Agent { session_id, .. } => session_id.clone(),
                    _ => None,
                },
            };
        }
        return author;
    }

    if let Some(author) = check_env_vars() {
        if interactive {
            return CommitAuthor::Assisted {
                tool: match &author {
                    CommitAuthor::Agent { tool, .. } => tool.clone(),
                    _ => "unknown".into(),
                },
                session_id: None,
            };
        }
        return author;
    }

    if interactive {
        CommitAuthor::Human
    } else {
        CommitAuthor::Automated
    }
}

/// Check for active session state files (worktree-aware).
/// Uses the shared state module which resolves the git common dir
/// and filters sessions to the current worktree.
fn check_active_sessions(project_root: &str) -> Option<CommitAuthor> {
    let sessions = crate::hooks::state::active_sessions_for_worktree(project_root);
    let first = sessions.into_iter().next()?;
    Some(CommitAuthor::Agent {
        tool: first.agent,
        session_id: Some(first.session_id),
    })
}

/// Check for environment variables that indicate an agent context.
fn check_env_vars() -> Option<CommitAuthor> {
    if std::env::var("CURSOR_TRACE_ID").is_ok() {
        return Some(CommitAuthor::Agent {
            tool: "cursor".to_string(),
            session_id: None,
        });
    }

    if std::env::var("CI").is_ok() {
        return Some(CommitAuthor::Automated);
    }

    None
}

/// Check if stdin is connected to a terminal (interactive human session).
pub fn is_interactive() -> bool {
    std::io::stdin().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    /// Tests that manipulate process-wide environment variables must hold this
    /// lock so they don't race against each other when `cargo test` runs them
    /// in parallel.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_commit_author_variants() {
        let agent = CommitAuthor::Agent {
            tool: "cursor".into(),
            session_id: Some("abc-123".into()),
        };
        assert_eq!(
            agent,
            CommitAuthor::Agent {
                tool: "cursor".into(),
                session_id: Some("abc-123".into()),
            }
        );
        assert_ne!(agent, CommitAuthor::Human);
        assert_ne!(CommitAuthor::Human, CommitAuthor::Automated);
    }

    #[test]
    fn test_detect_with_active_session() {
        let _guard = ENV_LOCK.lock().unwrap();

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        init_git_repo(root);

        let sessions_dir = root.join(".git/oobo-sessions");
        fs::create_dir_all(&sessions_dir).unwrap();

        let now = chrono::Utc::now().timestamp();
        let session_json = serde_json::json!({
            "session_id": "sess-1",
            "agent": "cursor",
            "worktree": root.to_str().unwrap(),
            "started_at": now,
            "updated_at": now,
        });
        fs::write(
            sessions_dir.join("test.json"),
            serde_json::to_string(&session_json).unwrap(),
        )
        .unwrap();

        let saved_cursor = std::env::var("CURSOR_TRACE_ID").ok();
        std::env::remove_var("CURSOR_TRACE_ID");

        let result = detect(root.to_str().unwrap());

        if let Some(v) = saved_cursor {
            std::env::set_var("CURSOR_TRACE_ID", v);
        }

        assert_eq!(
            result,
            CommitAuthor::Agent {
                tool: "cursor".to_string(),
                session_id: Some("sess-1".to_string()),
            }
        );
    }

    #[test]
    fn test_detect_no_sessions_no_env() {
        let _guard = ENV_LOCK.lock().unwrap();

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join(".git")).unwrap();

        let saved_cursor = std::env::var("CURSOR_TRACE_ID").ok();
        let saved_ci = std::env::var("CI").ok();
        std::env::remove_var("CURSOR_TRACE_ID");
        std::env::remove_var("CI");

        let result = detect(root.to_str().unwrap());

        if let Some(v) = saved_cursor {
            std::env::set_var("CURSOR_TRACE_ID", v);
        }
        if let Some(v) = saved_ci {
            std::env::set_var("CI", v);
        }

        match result {
            CommitAuthor::Human | CommitAuthor::Automated => {}
            other => panic!("Expected Human or Automated, got {other:?}"),
        }
    }

    #[test]
    fn test_detect_env_cursor_trace() {
        let _guard = ENV_LOCK.lock().unwrap();

        let saved = std::env::var("CURSOR_TRACE_ID").ok();

        std::env::set_var("CURSOR_TRACE_ID", "trace-abc-123");
        let result = check_env_vars();

        if let Some(v) = saved {
            std::env::set_var("CURSOR_TRACE_ID", v);
        } else {
            std::env::remove_var("CURSOR_TRACE_ID");
        }

        assert_eq!(
            result,
            Some(CommitAuthor::Agent {
                tool: "cursor".to_string(),
                session_id: None,
            })
        );
    }

    fn init_git_repo(root: &std::path::Path) {
        std::process::Command::new("git")
            .args(["init", "--initial-branch=main"])
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
    }

    #[test]
    fn test_check_active_sessions_empty_dir() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        init_git_repo(root);
        fs::create_dir_all(root.join(".git/oobo-sessions")).unwrap();

        let result = check_active_sessions(root.to_str().unwrap());
        assert_eq!(result, None);
    }

    #[test]
    fn test_check_active_sessions_invalid_json() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        init_git_repo(root);
        let sessions_dir = root.join(".git/oobo-sessions");
        fs::create_dir_all(&sessions_dir).unwrap();
        fs::write(sessions_dir.join("bad.json"), r#"not valid json"#).unwrap();

        let result = check_active_sessions(root.to_str().unwrap());
        assert_eq!(result, None);
    }
}
