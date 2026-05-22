use crate::core::tool::{Tool, ToolRegistry};

mod adapter;

pub mod aider;
pub mod claude;
pub mod codex;
pub mod continue_dev;
pub mod copilot;
pub mod cursor;
pub mod droid;
pub mod gemini;
pub mod opencode;
pub mod vscode_fork;
pub mod zed;

pub mod contrib;

pub use adapter::*;

/// Build the registry with all supported tools.
///
/// Returns the 10 first-class tools by default. When `tools.experimental` is
/// enabled in `~/.oobo/config.toml`, the 5 contrib adapters (Windsurf, Trae,
/// Amp, Junie, Kiro) are registered alongside them.
pub fn registry() -> ToolRegistry {
    registry_with(crate::config::Config::load_or_default().tools.experimental)
}

/// Build a registry, explicitly controlling whether experimental/contrib
/// adapters are included. Intended for tests and internal callers that need
/// a deterministic tool set.
pub fn registry_with(experimental: bool) -> ToolRegistry {
    let mut tools: Vec<Box<dyn Tool>> = vec![
        Box::new(CursorTool),
        Box::new(ClaudeTool),
        Box::new(GeminiTool),
        Box::new(AiderTool),
        Box::new(CopilotTool),
        Box::new(CodexTool),
        Box::new(OpenCodeTool),
        Box::new(ZedTool),
        Box::new(ContinueTool),
        Box::new(DroidTool),
    ];

    if experimental {
        tools.push(Box::new(WindsurfTool));
        tools.push(Box::new(TraeTool));
        tools.push(Box::new(KiroTool));
        tools.push(Box::new(JunieTool));
        tools.push(Box::new(AmpTool));
    }

    ToolRegistry::new(tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_registry_is_first_class_only() {
        let reg = registry_with(false);
        let names = reg.tool_names();
        let expected = [
            "cursor", "claude", "gemini", "aider", "copilot", "codex", "opencode", "zed",
            "continue", "droid",
        ];
        assert_eq!(
            names.len(),
            expected.len(),
            "first-class registry has {} tools, expected {}: {:?}",
            names.len(),
            expected.len(),
            names,
        );
    }

    #[test]
    fn test_experimental_registry_adds_contrib_tools() {
        let reg = registry_with(true);
        let names = reg.tool_names();
        assert_eq!(
            names.len(),
            15,
            "experimental registry should have 15 tools, got {names:?}"
        );
        for contrib in ["windsurf", "trae", "kiro", "junie", "amp"] {
            assert!(
                reg.by_name(contrib).is_some(),
                "experimental registry missing {contrib}"
            );
        }
    }

    #[test]
    fn test_registry_names_unique() {
        let reg = registry_with(true);
        let names: Vec<&str> = reg
            .all()
            .map(super::super::core::tool::Tool::name)
            .collect();
        let mut deduped = names.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(names.len(), deduped.len(), "duplicate tool names found");
    }

    #[test]
    fn test_registry_display_names_not_empty() {
        let reg = registry_with(true);
        for tool in reg.all() {
            assert!(
                !tool.display_name().is_empty(),
                "{} has empty display_name",
                tool.name()
            );
        }
    }

    #[test]
    fn test_registry_by_name_cursor_alias() {
        let reg = registry_with(false);
        // CursorTool::name() returns "composer" but config_key() returns "cursor".
        // by_name should find it via either.
        assert!(reg.by_name("composer").is_some());
        assert!(reg.by_name("cursor").is_some());
        assert_eq!(
            reg.by_name("composer").unwrap().display_name(),
            reg.by_name("cursor").unwrap().display_name()
        );
    }

    #[test]
    fn test_registry_by_name_all_config_keys() {
        let reg = registry_with(true);
        let keys = [
            "cursor", "claude", "gemini", "windsurf", "aider", "copilot", "codex", "opencode",
            "trae", "zed", "kiro", "continue", "droid", "junie", "amp",
        ];
        for key in keys {
            assert!(
                reg.by_name(key).is_some(),
                "by_name({key:?}) should find a tool"
            );
        }
    }

    // ── Adapter contract tests ────────────────────────────────────────────
    //
    // Verify that every registered tool adheres to the minimum contract:
    // identity methods return non-empty values, data methods return valid
    // shapes (not panics), and enabled() works against a default config.

    #[test]
    fn test_contract_identity_fields() {
        let reg = registry_with(true);
        for tool in reg.all() {
            let n = tool.name();
            assert!(!n.is_empty(), "name() must be non-empty");
            assert!(!tool.display_name().is_empty(), "{n}: display_name() empty");
            assert!(!tool.config_key().is_empty(), "{n}: config_key() empty");
            let cat = tool.category();
            assert!(
                cat == "ide" || cat == "cli",
                "{n}: category() must be 'ide' or 'cli', got {cat:?}"
            );
        }
    }

    #[test]
    fn test_contract_enabled_default_config() {
        let cfg = crate::config::Config::default();
        let reg = registry_with(true);
        for tool in reg.all() {
            // Must not panic, even with a default config.
            let _enabled = tool.enabled(&cfg);
        }
    }

    #[test]
    fn test_contract_sessions_do_not_panic() {
        let reg = registry_with(true);
        for tool in reg.all() {
            // all_sessions may fail (no artifacts on disk) but must not panic.
            let _ = tool.all_sessions();
        }
    }

    #[test]
    fn test_contract_sessions_for_nonexistent_project() {
        let reg = registry_with(true);
        for tool in reg.all() {
            let result = tool.sessions_for_project("/nonexistent/path");
            // Errors are acceptable when the native tool store is unavailable.
            if let Ok(sessions) = result {
                assert!(
                    sessions.is_empty(),
                    "{}: sessions_for_project on nonexistent path should be empty",
                    tool.name()
                );
            }
        }
    }

    #[test]
    fn test_contract_transcript_methods_do_not_panic() {
        let reg = registry_with(true);
        let dummy = std::path::Path::new("/nonexistent/transcript.jsonl");
        for tool in reg.all() {
            let n = tool.name();
            // find_transcript for nonexistent session — must not panic.
            let _ = tool.find_transcript("/nonexistent", "nonexistent-session-id");
            // parse_messages on a nonexistent path — must not panic.
            let msgs = tool.parse_messages(dummy);
            assert!(
                msgs.is_empty(),
                "{n}: parse_messages on bad path should be empty"
            );
        }
    }

    #[test]
    fn test_contract_extract_native_stats_does_not_panic() {
        use crate::core::session::Session;
        let reg = registry_with(true);
        let dummy_session = Session {
            session_id: "dummy".to_string(),
            name: "dummy".to_string(),
            mode: "agent".to_string(),
            created_at: Some(0),
            updated_at: Some(0),
            project_path: "/nonexistent".to_string(),
            workspace_dir: String::new(),
            source: "dummy".to_string(),
            parent_session_id: None,
            subagent_type: None,
        };
        for tool in reg.all() {
            let _ = tool.extract_native_stats(&dummy_session);
        }
    }

    #[test]
    fn test_first_class_tools_have_10_members() {
        let reg = registry_with(false);
        let count = reg.all().count();
        assert_eq!(
            count, 10,
            "first-class tools should be exactly 10, got {count}"
        );
    }
}
