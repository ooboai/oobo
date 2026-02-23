use std::path::PathBuf;

use crate::claude;
use crate::cursor;
use crate::cursor::transcript::Message;
use crate::cursor::Session;

/// Find the transcript path for a session, dispatching based on source.
pub fn find_transcript_path(session: &Session) -> Option<PathBuf> {
    match session.source.as_str() {
        "claude" => {
            claude::transcript::find_transcript_path(&session.project_path, &session.session_id)
        }
        "windsurf" => crate::windsurf::transcript::find_transcript_path(
            &session.project_path,
            &session.session_id,
        ),
        "trae" => crate::trae::transcript::find_transcript_path(
            &session.project_path,
            &session.session_id,
        ),
        "aider" => crate::aider::transcript::find_transcript_path(
            &session.project_path,
            &session.session_id,
        ),
        "continue" => crate::continue_dev::transcript::find_transcript_path(
            &session.project_path,
            &session.session_id,
        ),
        "copilot" => crate::copilot::transcript::find_transcript_path(
            &session.project_path,
            &session.session_id,
        ),
        "zed" => {
            crate::zed::transcript::find_transcript_path(&session.project_path, &session.session_id)
        }
        "codex" => crate::codex::transcript::find_transcript_path(
            &session.project_path,
            &session.session_id,
        ),
        _ => cursor::transcript::find_transcript_path(&session.project_path, &session.session_id),
    }
}

/// Count messages in a session's transcript, dispatching based on source.
pub fn count_messages(session: &Session) -> u32 {
    match session.source.as_str() {
        "claude" => claude::transcript::count_messages(&session.project_path, &session.session_id),
        "windsurf" => {
            crate::windsurf::transcript::count_messages(&session.project_path, &session.session_id)
        }
        "trae" => {
            crate::trae::transcript::count_messages(&session.project_path, &session.session_id)
        }
        "aider" => {
            crate::aider::transcript::count_messages(&session.project_path, &session.session_id)
        }
        "continue" => crate::continue_dev::transcript::count_messages(
            &session.project_path,
            &session.session_id,
        ),
        "copilot" => {
            crate::copilot::transcript::count_messages(&session.project_path, &session.session_id)
        }
        "zed" => crate::zed::transcript::count_messages(&session.project_path, &session.session_id),
        "codex" => {
            crate::codex::transcript::count_messages(&session.project_path, &session.session_id)
        }
        _ => cursor::transcript::count_messages(&session.project_path, &session.session_id),
    }
}

/// Parse messages from a transcript file, dispatching based on source.
pub fn parse_messages(path: &std::path::Path, source: &str) -> Vec<Message> {
    match source {
        "claude" => claude::transcript::parse_messages(path),
        "windsurf" => crate::windsurf::transcript::parse_messages(path),
        "trae" => crate::trae::transcript::parse_messages(path),
        "aider" => crate::aider::transcript::parse_messages(path),
        "continue" => crate::continue_dev::transcript::parse_messages(path),
        "copilot" => crate::copilot::transcript::parse_messages(path),
        "zed" => crate::zed::transcript::parse_messages(path),
        "codex" => crate::codex::transcript::parse_messages(path),
        _ => cursor::transcript::parse_messages(path),
    }
}

/// Read transcript as formatted text, dispatching based on source.
pub fn read_transcript(path: &std::path::Path, max_messages: u32, source: &str) -> String {
    match source {
        "claude" => claude::transcript::read_transcript(path, max_messages),
        "windsurf" => crate::windsurf::transcript::read_transcript(path, max_messages),
        "trae" => crate::trae::transcript::read_transcript(path, max_messages),
        "aider" => crate::aider::transcript::read_transcript(path, max_messages),
        "continue" => crate::continue_dev::transcript::read_transcript(path, max_messages),
        "copilot" => crate::copilot::transcript::read_transcript(path, max_messages),
        "zed" => crate::zed::transcript::read_transcript(path, max_messages),
        "codex" => crate::codex::transcript::read_transcript(path, max_messages),
        _ => cursor::transcript::read_transcript(path, max_messages),
    }
}

/// Find a session by ID prefix across all sources.
pub fn find_session_any(id_prefix: &str) -> Result<Session, String> {
    let project_root = cursor::get_project_root();

    macro_rules! try_source {
        ($result:expr) => {
            if let Ok(sessions) = $result {
                if let Some(s) = sessions
                    .iter()
                    .find(|s| s.session_id.starts_with(id_prefix))
                {
                    return Ok(s.clone());
                }
            }
        };
    }

    try_source!(cursor::sessions_for_project(&project_root));
    try_source!(claude::sessions_for_project(&project_root));
    try_source!(crate::windsurf::sessions_for_project(&project_root));
    try_source!(crate::trae::sessions_for_project(&project_root));
    try_source!(crate::aider::sessions_for_project(&project_root));
    try_source!(crate::continue_dev::sessions_for_project(&project_root));
    try_source!(crate::copilot::sessions_for_project(&project_root));
    try_source!(crate::zed::sessions_for_project(&project_root));
    try_source!(crate::codex::sessions_for_project(&project_root));

    try_source!(cursor::all_sessions());
    try_source!(claude::all_sessions());
    try_source!(crate::windsurf::all_sessions());
    try_source!(crate::trae::all_sessions());
    try_source!(crate::continue_dev::all_sessions());
    try_source!(crate::copilot::all_sessions());
    try_source!(crate::zed::all_sessions());
    try_source!(crate::codex::all_sessions());

    Err(format!("session not found: {id_prefix}"))
}

/// Get all sessions across all sources for the current project.
pub fn all_for_project(project_root: &str, cfg: &crate::config::Config) -> Vec<Session> {
    let mut sessions = Vec::new();

    macro_rules! collect {
        ($enabled:expr, $result:expr) => {
            if $enabled {
                sessions.extend($result.unwrap_or_default());
            }
        };
    }

    collect!(
        cfg.cursor.enabled,
        cursor::sessions_for_project(project_root)
    );
    collect!(
        cfg.claude.enabled,
        claude::sessions_for_project(project_root)
    );
    collect!(
        cfg.windsurf.enabled,
        crate::windsurf::sessions_for_project(project_root)
    );
    collect!(
        cfg.trae.enabled,
        crate::trae::sessions_for_project(project_root)
    );
    collect!(
        cfg.aider.enabled,
        crate::aider::sessions_for_project(project_root)
    );
    collect!(
        cfg.continue_dev.enabled,
        crate::continue_dev::sessions_for_project(project_root)
    );
    collect!(
        cfg.copilot.enabled,
        crate::copilot::sessions_for_project(project_root)
    );
    collect!(
        cfg.zed.enabled,
        crate::zed::sessions_for_project(project_root)
    );
    collect!(
        cfg.codex.enabled,
        crate::codex::sessions_for_project(project_root)
    );

    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    sessions
}

/// Get all sessions across all sources and all projects.
pub fn all_sessions(cfg: &crate::config::Config) -> Vec<Session> {
    let mut sessions = Vec::new();
    let project_root = cursor::get_project_root();

    macro_rules! collect {
        ($enabled:expr, $result:expr) => {
            if $enabled {
                sessions.extend($result.unwrap_or_default());
            }
        };
    }

    collect!(cfg.cursor.enabled, cursor::all_sessions());
    collect!(cfg.claude.enabled, claude::all_sessions());
    collect!(cfg.windsurf.enabled, crate::windsurf::all_sessions());
    collect!(cfg.trae.enabled, crate::trae::all_sessions());
    collect!(
        cfg.aider.enabled,
        crate::aider::sessions_for_project(&project_root)
    );
    collect!(
        cfg.continue_dev.enabled,
        crate::continue_dev::all_sessions()
    );
    collect!(cfg.copilot.enabled, crate::copilot::all_sessions());
    collect!(cfg.zed.enabled, crate::zed::all_sessions());
    collect!(cfg.codex.enabled, crate::codex::all_sessions());

    sessions.sort_by_key(|s| std::cmp::Reverse(s.sort_key()));
    sessions
}
