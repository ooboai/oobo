//! File-based turn cache.
//!
//! Turns are cached as JSON files under `.oobo/cache/turns/<source>/<session_id>.json`.
//! This is a local working cache  --  rebuildable from native AI tool artifacts.

use crate::core::turn::Turn;
use crate::taps::{SubagentLink, TurnSink};
use std::path::{Path, PathBuf};

fn cache_dir(project_root: &str) -> PathBuf {
    Path::new(project_root)
        .join(".oobo")
        .join("cache")
        .join("turns")
}

fn session_path(project_root: &str, source: &str, session_id: &str) -> PathBuf {
    cache_dir(project_root)
        .join(sanitize_segment(source))
        .join(format!("{}.json", sanitize_segment(session_id)))
}

fn sanitize_segment(raw: &str) -> String {
    if raw.is_empty() {
        return "invalid".to_string();
    }
    let safe: String = raw
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c,
            _ => '_',
        })
        .collect();
    let cleaned = safe.replace("..", "_");
    let trimmed = cleaned.trim_matches('.').to_string();
    if trimmed.is_empty() {
        "invalid".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
pub fn read_turns(project_root: &str, source: &str, session_id: &str) -> Vec<Turn> {
    let path = session_path(project_root, source, session_id);
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

pub fn write_turns(
    project_root: &str,
    source: &str,
    session_id: &str,
    turns: &[Turn],
) -> Result<(), String> {
    let path = session_path(project_root, source, session_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create turn cache dir: {e}"))?;
    }
    let json = serde_json::to_string(turns).map_err(|e| format!("serialize turns: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("write turn cache: {e}"))?;
    Ok(())
}

pub fn read_all_turns(project_root: &str) -> Vec<Turn> {
    let dir = cache_dir(project_root);
    let mut all = Vec::new();
    if let Ok(sources) = std::fs::read_dir(&dir) {
        for source_entry in sources.flatten() {
            if !source_entry
                .file_type()
                .map(|t| t.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            if let Ok(sessions) = std::fs::read_dir(source_entry.path()) {
                for session_entry in sessions.flatten() {
                    let path = session_entry.path();
                    if path.extension().and_then(|s| s.to_str()) != Some("json") {
                        continue;
                    }
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Ok(turns) = serde_json::from_str::<Vec<Turn>>(&content) {
                            all.extend(turns);
                        }
                    }
                }
            }
        }
    }
    all
}

/// A tap turn's timestamp in unix **seconds**.
///
/// Taps store milliseconds when the native artifact carries RFC3339
/// timestamps (Claude, Codex) and seconds otherwise; normalize so
/// time-window joins against hook-recorded times (always seconds) work.
pub fn turn_ts_secs(turn: &Turn) -> Option<i64> {
    let ts = turn.started_at.or(turn.ended_at)?;
    Some(if ts.abs() >= 100_000_000_000 {
        ts / 1000
    } else {
        ts
    })
}

/// A TurnSink that collects turns and subagent links in memory.
#[derive(Default)]
pub struct CollectingSink {
    pub turns: Vec<Turn>,
    pub subagent_links: Vec<SubagentLink>,
}

impl CollectingSink {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TurnSink for CollectingSink {
    fn accept_turn(&mut self, turn: Turn) {
        self.turns.push(turn);
    }
    fn accept_subagent_link(&mut self, link: SubagentLink) {
        self.subagent_links.push(link);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::turn::{TurnRole, TurnTokens};

    fn make_turn(session_id: &str, source: &str, idx: i64) -> Turn {
        Turn {
            id: Turn::deterministic_id(source, session_id, idx),
            session_id: session_id.to_string(),
            source: source.to_string(),
            turn_index: idx,
            role: TurnRole::Assistant,
            started_at: Some(1000 + idx),
            ended_at: Some(1001 + idx),
            model: None,
            tokens: TurnTokens {
                input: Some(10),
                output: Some(20),
                ..Default::default()
            },
            cost_usd: None,
            tool_call_count: 0,
            thinking_ms: None,
            message_preview: None,
            raw_ref: None,
            tool_names: None,
        }
    }

    #[test]
    fn round_trip_write_and_read() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_str().unwrap();

        let turns = vec![make_turn("s1", "claude", 0), make_turn("s1", "claude", 1)];
        write_turns(root, "claude", "s1", &turns).unwrap();
        let loaded = read_turns(root, "claude", "s1");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].session_id, "s1");
        assert_eq!(loaded[1].turn_index, 1);
    }

    #[test]
    fn read_missing_returns_empty() {
        let loaded = read_turns("/nonexistent/path", "claude", "s1");
        assert!(loaded.is_empty());
    }

    #[test]
    fn read_all_turns_across_sources() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_str().unwrap();

        write_turns(root, "claude", "s1", &[make_turn("s1", "claude", 0)]).unwrap();
        write_turns(root, "codex", "s2", &[make_turn("s2", "codex", 0)]).unwrap();

        let all = read_all_turns(root);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn collecting_sink_collects() {
        let mut sink = CollectingSink::new();
        sink.accept_turn(make_turn("s1", "claude", 0));
        sink.accept_subagent_link(SubagentLink {
            child_session_id: "child".into(),
            child_source: "claude".into(),
            parent_session_id: "parent".into(),
            parent_source: "claude".into(),
            parent_turn_id: None,
            subagent_kind: Some("task".into()),
        });
        assert_eq!(sink.turns.len(), 1);
        assert_eq!(sink.subagent_links.len(), 1);
    }
}
