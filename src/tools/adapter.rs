use std::path::{Path, PathBuf};

use crate::analytics::NativeStats;
use crate::config::Config;
use crate::core::message::Message;
use crate::core::session::Session;
use crate::core::tool::Tool;
use crate::remote::payload::SessionStats;

fn payload_to_native(s: SessionStats) -> NativeStats {
    NativeStats {
        model: s.model,
        input_tokens: s.input_tokens,
        output_tokens: s.output_tokens,
        cache_read_tokens: s.cache_read_tokens,
        cache_creation_tokens: s.cache_creation_tokens,
        duration_secs: s.duration_secs,
        files_touched: s.files_touched,
        tool_call_count: s.tool_call_count,
    }
}

// Cursor

pub struct CursorTool;

impl Tool for CursorTool {
    fn name(&self) -> &'static str {
        "composer"
    }
    fn display_name(&self) -> &'static str {
        "Cursor"
    }
    fn config_key(&self) -> &'static str {
        "cursor"
    }
    fn category(&self) -> &'static str {
        "ide"
    }
    fn enabled(&self, cfg: &Config) -> bool {
        cfg.cursor.enabled
    }

    fn sessions_for_project(&self, project_path: &str) -> Result<Vec<Session>, String> {
        crate::tools::cursor::sessions_for_project(project_path)
    }

    fn all_sessions(&self) -> Result<Vec<Session>, String> {
        crate::tools::cursor::all_sessions()
    }

    fn find_transcript(&self, project_path: &str, session_id: &str) -> Option<PathBuf> {
        crate::tools::cursor::transcript::find_transcript_path(project_path, session_id)
    }

    fn parse_messages(&self, path: &Path) -> Vec<Message> {
        crate::tools::cursor::transcript::parse_messages(path)
    }

    fn count_messages(&self, project_path: &str, session_id: &str) -> u32 {
        crate::tools::cursor::transcript::count_messages(project_path, session_id)
    }

    fn read_transcript(&self, path: &Path, max_messages: u32) -> String {
        crate::tools::cursor::transcript::read_transcript(path, max_messages)
    }

    fn extract_native_stats(&self, session: &Session) -> Option<NativeStats> {
        crate::tools::cursor::composer_data::extract_native_stats(&session.session_id)
    }
}

// Claude

pub struct ClaudeTool;

impl Tool for ClaudeTool {
    fn name(&self) -> &'static str {
        "claude"
    }
    fn display_name(&self) -> &'static str {
        "Claude Code"
    }
    fn config_key(&self) -> &'static str {
        "claude"
    }
    fn category(&self) -> &'static str {
        "cli"
    }
    fn enabled(&self, cfg: &Config) -> bool {
        cfg.claude.enabled
    }

    fn sessions_for_project(&self, project_path: &str) -> Result<Vec<Session>, String> {
        crate::tools::claude::sessions_for_project(project_path)
    }

    fn all_sessions(&self) -> Result<Vec<Session>, String> {
        crate::tools::claude::all_sessions()
    }

    fn find_transcript(&self, project_path: &str, session_id: &str) -> Option<PathBuf> {
        crate::tools::claude::transcript::find_transcript_path(project_path, session_id)
    }

    fn parse_messages(&self, path: &Path) -> Vec<Message> {
        crate::tools::claude::transcript::parse_messages(path)
    }

    fn count_messages(&self, project_path: &str, session_id: &str) -> u32 {
        crate::tools::claude::transcript::count_messages(project_path, session_id)
    }

    fn read_transcript(&self, path: &Path, max_messages: u32) -> String {
        crate::tools::claude::transcript::read_transcript(path, max_messages)
    }

    fn extract_native_stats(&self, session: &Session) -> Option<NativeStats> {
        crate::tools::claude::transcript::extract_native_stats(
            &session.project_path,
            &session.session_id,
        )
    }
}

// Gemini

pub struct GeminiTool;

impl Tool for GeminiTool {
    fn name(&self) -> &'static str {
        "gemini"
    }
    fn display_name(&self) -> &'static str {
        "Gemini CLI"
    }
    fn config_key(&self) -> &'static str {
        "gemini"
    }
    fn category(&self) -> &'static str {
        "cli"
    }
    fn enabled(&self, cfg: &Config) -> bool {
        cfg.gemini.enabled
    }

    fn sessions_for_project(&self, project_path: &str) -> Result<Vec<Session>, String> {
        crate::tools::gemini::sessions_for_project(project_path)
    }

    fn all_sessions(&self) -> Result<Vec<Session>, String> {
        crate::tools::gemini::all_sessions()
    }

    fn find_transcript(&self, project_path: &str, session_id: &str) -> Option<PathBuf> {
        crate::tools::gemini::transcript::find_transcript_path(project_path, session_id)
    }

    fn parse_messages(&self, path: &Path) -> Vec<Message> {
        crate::tools::gemini::transcript::parse_messages(path)
    }

    fn count_messages(&self, project_path: &str, session_id: &str) -> u32 {
        crate::tools::gemini::transcript::count_messages(project_path, session_id)
    }

    fn read_transcript(&self, path: &Path, max_messages: u32) -> String {
        crate::tools::gemini::transcript::read_transcript(path, max_messages)
    }

    fn extract_native_stats(&self, session: &Session) -> Option<NativeStats> {
        let stats = crate::tools::gemini::transcript::stats_for_session(
            &session.project_path,
            &session.session_id,
        )?;
        Some(payload_to_native(stats))
    }
}

// Windsurf

pub struct WindsurfTool;

impl Tool for WindsurfTool {
    fn name(&self) -> &'static str {
        "windsurf"
    }
    fn display_name(&self) -> &'static str {
        "Windsurf"
    }
    fn config_key(&self) -> &'static str {
        "windsurf"
    }
    fn category(&self) -> &'static str {
        "ide"
    }
    fn enabled(&self, cfg: &Config) -> bool {
        cfg.windsurf.enabled
    }

    fn sessions_for_project(&self, project_path: &str) -> Result<Vec<Session>, String> {
        crate::tools::windsurf::sessions_for_project(project_path)
    }

    fn all_sessions(&self) -> Result<Vec<Session>, String> {
        crate::tools::windsurf::all_sessions()
    }

    fn find_transcript(&self, project_path: &str, session_id: &str) -> Option<PathBuf> {
        crate::tools::windsurf::transcript::find_transcript_path(project_path, session_id)
    }

    fn parse_messages(&self, path: &Path) -> Vec<Message> {
        crate::tools::windsurf::transcript::parse_messages(path)
    }

    fn count_messages(&self, project_path: &str, session_id: &str) -> u32 {
        crate::tools::windsurf::transcript::count_messages(project_path, session_id)
    }

    fn read_transcript(&self, path: &Path, max_messages: u32) -> String {
        crate::tools::windsurf::transcript::read_transcript(path, max_messages)
    }
}

// Aider

pub struct AiderTool;

impl Tool for AiderTool {
    fn name(&self) -> &'static str {
        "aider"
    }
    fn display_name(&self) -> &'static str {
        "Aider"
    }
    fn config_key(&self) -> &'static str {
        "aider"
    }
    fn category(&self) -> &'static str {
        "cli"
    }
    fn enabled(&self, cfg: &Config) -> bool {
        cfg.aider.enabled
    }

    fn sessions_for_project(&self, project_path: &str) -> Result<Vec<Session>, String> {
        crate::tools::aider::sessions_for_project(project_path)
    }

    fn all_sessions(&self) -> Result<Vec<Session>, String> {
        let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
        let project_root = std::process::Command::new(git)
            .args(["rev-parse", "--show-toplevel"])
            .stdin(std::process::Stdio::null())
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_QUARANTINE_PATH")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default()
            });
        crate::tools::aider::sessions_for_project(&project_root)
    }

    fn find_transcript(&self, project_path: &str, session_id: &str) -> Option<PathBuf> {
        crate::tools::aider::transcript::find_transcript_path(project_path, session_id)
    }

    fn parse_messages(&self, path: &Path) -> Vec<Message> {
        crate::tools::aider::transcript::parse_messages(path)
    }

    fn count_messages(&self, project_path: &str, session_id: &str) -> u32 {
        crate::tools::aider::transcript::count_messages(project_path, session_id)
    }

    fn read_transcript(&self, path: &Path, max_messages: u32) -> String {
        crate::tools::aider::transcript::read_transcript(path, max_messages)
    }

    fn extract_native_stats(&self, session: &Session) -> Option<NativeStats> {
        let end = if session.updated_at != session.created_at {
            session.updated_at
        } else {
            None
        };
        crate::tools::aider::analytics::extract_native_stats(session.created_at, end)
    }
}

// Copilot

pub struct CopilotTool;

impl Tool for CopilotTool {
    fn name(&self) -> &'static str {
        "copilot"
    }
    fn display_name(&self) -> &'static str {
        "Copilot"
    }
    fn config_key(&self) -> &'static str {
        "copilot"
    }
    fn category(&self) -> &'static str {
        "ide"
    }
    fn enabled(&self, cfg: &Config) -> bool {
        cfg.copilot.enabled
    }

    fn sessions_for_project(&self, project_path: &str) -> Result<Vec<Session>, String> {
        crate::tools::copilot::sessions_for_project(project_path)
    }

    fn all_sessions(&self) -> Result<Vec<Session>, String> {
        crate::tools::copilot::all_sessions()
    }

    fn find_transcript(&self, project_path: &str, session_id: &str) -> Option<PathBuf> {
        crate::tools::copilot::transcript::find_transcript_path(project_path, session_id)
    }

    fn parse_messages(&self, path: &Path) -> Vec<Message> {
        crate::tools::copilot::transcript::parse_messages(path)
    }

    fn count_messages(&self, project_path: &str, session_id: &str) -> u32 {
        crate::tools::copilot::transcript::count_messages(project_path, session_id)
    }

    fn read_transcript(&self, path: &Path, max_messages: u32) -> String {
        crate::tools::copilot::transcript::read_transcript(path, max_messages)
    }

    fn extract_native_stats(&self, session: &Session) -> Option<NativeStats> {
        let stats = crate::tools::copilot::transcript::stats_for_session(
            &session.project_path,
            &session.session_id,
        )?;
        Some(payload_to_native(stats))
    }
}

// Codex

pub struct CodexTool;

impl Tool for CodexTool {
    fn name(&self) -> &'static str {
        "codex"
    }
    fn display_name(&self) -> &'static str {
        "Codex CLI"
    }
    fn config_key(&self) -> &'static str {
        "codex"
    }
    fn category(&self) -> &'static str {
        "cli"
    }
    fn enabled(&self, cfg: &Config) -> bool {
        cfg.codex.enabled
    }

    fn sessions_for_project(&self, project_path: &str) -> Result<Vec<Session>, String> {
        crate::tools::codex::sessions_for_project(project_path)
    }

    fn all_sessions(&self) -> Result<Vec<Session>, String> {
        crate::tools::codex::all_sessions()
    }

    fn find_transcript(&self, project_path: &str, session_id: &str) -> Option<PathBuf> {
        crate::tools::codex::transcript::find_transcript_path(project_path, session_id)
    }

    fn parse_messages(&self, path: &Path) -> Vec<Message> {
        crate::tools::codex::transcript::parse_messages(path)
    }

    fn count_messages(&self, project_path: &str, session_id: &str) -> u32 {
        crate::tools::codex::transcript::count_messages(project_path, session_id)
    }

    fn read_transcript(&self, path: &Path, max_messages: u32) -> String {
        crate::tools::codex::transcript::read_transcript(path, max_messages)
    }

    fn extract_native_stats(&self, session: &Session) -> Option<NativeStats> {
        let stats = crate::tools::codex::transcript::stats_for_session(
            &session.project_path,
            &session.session_id,
        )?;
        Some(payload_to_native(stats))
    }
}

// OpenCode

pub struct OpenCodeTool;

impl Tool for OpenCodeTool {
    fn name(&self) -> &'static str {
        "opencode"
    }
    fn display_name(&self) -> &'static str {
        "OpenCode"
    }
    fn config_key(&self) -> &'static str {
        "opencode"
    }
    fn category(&self) -> &'static str {
        "cli"
    }
    fn enabled(&self, cfg: &Config) -> bool {
        cfg.opencode.enabled
    }

    fn sessions_for_project(&self, project_path: &str) -> Result<Vec<Session>, String> {
        crate::tools::opencode::sessions_for_project(project_path)
    }

    fn all_sessions(&self) -> Result<Vec<Session>, String> {
        crate::tools::opencode::all_sessions()
    }

    fn find_transcript(&self, project_path: &str, session_id: &str) -> Option<PathBuf> {
        crate::tools::opencode::transcript::find_transcript_path(project_path, session_id)
    }

    fn parse_messages(&self, path: &Path) -> Vec<Message> {
        crate::tools::opencode::transcript::parse_messages(path)
    }

    fn parse_messages_by_id(&self, _project_path: &str, session_id: &str) -> Vec<Message> {
        if let Some(db_path) = crate::tools::opencode::find_db_path() {
            crate::tools::opencode::transcript::parse_messages_for_session(&db_path, session_id)
        } else {
            Vec::new()
        }
    }

    fn count_messages(&self, project_path: &str, session_id: &str) -> u32 {
        crate::tools::opencode::transcript::count_messages(project_path, session_id)
    }

    fn read_transcript(&self, path: &Path, max_messages: u32) -> String {
        crate::tools::opencode::transcript::read_transcript(path, max_messages)
    }

    fn read_transcript_by_id(
        &self,
        _project_path: &str,
        session_id: &str,
        max_messages: u32,
    ) -> String {
        if let Some(db_path) = crate::tools::opencode::find_db_path() {
            crate::tools::opencode::transcript::read_transcript_for_session(
                &db_path,
                session_id,
                max_messages,
            )
        } else {
            String::new()
        }
    }

    fn extract_native_stats(&self, session: &Session) -> Option<NativeStats> {
        let stats = crate::tools::opencode::transcript::stats_for_session(
            &session.project_path,
            &session.session_id,
        )?;
        Some(payload_to_native(stats))
    }
}

// Trae

pub struct TraeTool;

impl Tool for TraeTool {
    fn name(&self) -> &'static str {
        "trae"
    }
    fn display_name(&self) -> &'static str {
        "Trae"
    }
    fn config_key(&self) -> &'static str {
        "trae"
    }
    fn category(&self) -> &'static str {
        "ide"
    }
    fn enabled(&self, cfg: &Config) -> bool {
        cfg.trae.enabled
    }

    fn sessions_for_project(&self, project_path: &str) -> Result<Vec<Session>, String> {
        crate::tools::trae::sessions_for_project(project_path)
    }

    fn all_sessions(&self) -> Result<Vec<Session>, String> {
        crate::tools::trae::all_sessions()
    }

    fn find_transcript(&self, project_path: &str, session_id: &str) -> Option<PathBuf> {
        crate::tools::trae::transcript::find_transcript_path(project_path, session_id)
    }

    fn parse_messages(&self, path: &Path) -> Vec<Message> {
        crate::tools::trae::transcript::parse_messages(path)
    }

    fn count_messages(&self, project_path: &str, session_id: &str) -> u32 {
        crate::tools::trae::transcript::count_messages(project_path, session_id)
    }

    fn read_transcript(&self, path: &Path, max_messages: u32) -> String {
        crate::tools::trae::transcript::read_transcript(path, max_messages)
    }
}

// Zed

pub struct ZedTool;

impl Tool for ZedTool {
    fn name(&self) -> &'static str {
        "zed"
    }
    fn display_name(&self) -> &'static str {
        "Zed"
    }
    fn config_key(&self) -> &'static str {
        "zed"
    }
    fn category(&self) -> &'static str {
        "ide"
    }
    fn enabled(&self, cfg: &Config) -> bool {
        cfg.zed.enabled
    }

    fn sessions_for_project(&self, project_path: &str) -> Result<Vec<Session>, String> {
        crate::tools::zed::sessions_for_project(project_path)
    }

    fn all_sessions(&self) -> Result<Vec<Session>, String> {
        crate::tools::zed::all_sessions()
    }

    fn find_transcript(&self, project_path: &str, session_id: &str) -> Option<PathBuf> {
        crate::tools::zed::transcript::find_transcript_path(project_path, session_id)
    }

    fn parse_messages(&self, path: &Path) -> Vec<Message> {
        crate::tools::zed::transcript::parse_messages(path)
    }

    fn count_messages(&self, project_path: &str, session_id: &str) -> u32 {
        crate::tools::zed::transcript::count_messages(project_path, session_id)
    }

    fn read_transcript(&self, path: &Path, max_messages: u32) -> String {
        crate::tools::zed::transcript::read_transcript(path, max_messages)
    }

    fn extract_native_stats(&self, session: &Session) -> Option<NativeStats> {
        crate::tools::zed::telemetry::extract_native_stats(&session.session_id)
    }
}
