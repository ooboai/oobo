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
        match &author {
            CommitAuthor::Agent { tool, .. } if interactive => {
                return CommitAuthor::Assisted {
                    tool: tool.clone(),
                    session_id: None,
                };
            }
            _ => return author,
        }
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

    if std::env::var("CLAUDECODE").is_ok() || std::env::var("CLAUDE_CODE_ENTRYPOINT").is_ok() {
        return Some(CommitAuthor::Agent {
            tool: "claude".to_string(),
            session_id: None,
        });
    }

    if std::env::var("CODEX_SANDBOX").is_ok() {
        return Some(CommitAuthor::Agent {
            tool: "codex".to_string(),
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

/// Resolve the worktree-specific git directory.
///
/// - Main worktree: `<repo>/.git`
/// - Linked worktree: `<main-repo>/.git/worktrees/<name>`
///
/// Use this for resources scoped to a single worktree (hooks, first-use marker).
pub fn resolve_git_dir(project_root: &str) -> std::path::PathBuf {
    resolve_git_path(project_root, "--git-dir")
}

/// Resolve the shared git directory that all worktrees share.
///
/// - Main worktree: `<repo>/.git`
/// - Linked worktree: `<main-repo>/.git` (the common dir)
///
/// Use this for shared resources (orphan branch index, push-pending marker, sessions).
pub fn resolve_git_common_dir(project_root: &str) -> std::path::PathBuf {
    resolve_git_path(project_root, "--git-common-dir")
}

fn resolve_git_path(project_root: &str, flag: &str) -> std::path::PathBuf {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let output = std::process::Command::new(git)
        .args(["rev-parse", flag])
        .current_dir(project_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_QUARANTINE_PATH")
        .output();

    if let Ok(o) = output {
        if o.status.success() {
            let raw = String::from_utf8_lossy(&o.stdout)
                .replace('\r', "")
                .trim()
                .to_string();
            let p = std::path::Path::new(&raw);
            if p.is_absolute() {
                return p.to_path_buf();
            }
            return std::path::Path::new(project_root).join(p);
        }
    }

    std::path::Path::new(project_root).join(".git")
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

    #[test]
    fn test_detect_env_claudecode() {
        let _guard = ENV_LOCK.lock().unwrap();

        let saved_cursor = std::env::var("CURSOR_TRACE_ID").ok();
        let saved_claude = std::env::var("CLAUDECODE").ok();
        std::env::remove_var("CURSOR_TRACE_ID");
        std::env::set_var("CLAUDECODE", "1");

        let result = check_env_vars();

        if let Some(v) = saved_cursor {
            std::env::set_var("CURSOR_TRACE_ID", v);
        }
        if let Some(v) = saved_claude {
            std::env::set_var("CLAUDECODE", v);
        } else {
            std::env::remove_var("CLAUDECODE");
        }

        assert_eq!(
            result,
            Some(CommitAuthor::Agent {
                tool: "claude".to_string(),
                session_id: None,
            })
        );
    }

    #[test]
    fn test_detect_env_codex_sandbox() {
        let _guard = ENV_LOCK.lock().unwrap();

        let saved_cursor = std::env::var("CURSOR_TRACE_ID").ok();
        let saved_claude = std::env::var("CLAUDECODE").ok();
        let saved_entrypoint = std::env::var("CLAUDE_CODE_ENTRYPOINT").ok();
        let saved_codex = std::env::var("CODEX_SANDBOX").ok();
        std::env::remove_var("CURSOR_TRACE_ID");
        std::env::remove_var("CLAUDECODE");
        std::env::remove_var("CLAUDE_CODE_ENTRYPOINT");
        std::env::set_var("CODEX_SANDBOX", "seatbelt");

        let result = check_env_vars();

        if let Some(v) = saved_cursor {
            std::env::set_var("CURSOR_TRACE_ID", v);
        }
        if let Some(v) = saved_claude {
            std::env::set_var("CLAUDECODE", v);
        }
        if let Some(v) = saved_entrypoint {
            std::env::set_var("CLAUDE_CODE_ENTRYPOINT", v);
        }
        if let Some(v) = saved_codex {
            std::env::set_var("CODEX_SANDBOX", v);
        } else {
            std::env::remove_var("CODEX_SANDBOX");
        }

        assert_eq!(
            result,
            Some(CommitAuthor::Agent {
                tool: "codex".to_string(),
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

    #[test]
    fn test_check_env_vars_ci_returns_automated() {
        let _guard = ENV_LOCK.lock().unwrap();

        let saved_ci = std::env::var("CI").ok();
        let saved_cursor = std::env::var("CURSOR_TRACE_ID").ok();
        let saved_claude = std::env::var("CLAUDECODE").ok();
        std::env::remove_var("CURSOR_TRACE_ID");
        std::env::remove_var("CLAUDECODE");
        std::env::remove_var("CLAUDE_CODE_ENTRYPOINT");
        std::env::remove_var("CODEX_SANDBOX");
        std::env::set_var("CI", "1");

        let result = check_env_vars();

        if let Some(v) = saved_ci {
            std::env::set_var("CI", v);
        } else {
            std::env::remove_var("CI");
        }
        if let Some(v) = saved_cursor {
            std::env::set_var("CURSOR_TRACE_ID", v);
        }
        if let Some(v) = saved_claude {
            std::env::set_var("CLAUDECODE", v);
        }

        assert_eq!(result, Some(CommitAuthor::Automated));
    }
}
