use crate::core::tool::ToolRegistry;

mod adapter;

pub mod aider;
pub mod amp;
pub mod claude;
pub mod codex;
pub mod continue_dev;
pub mod copilot;
pub mod cursor;
pub mod droid;
pub mod gemini;
pub mod junie;
pub mod kiro;
pub mod opencode;
pub mod trae;
pub mod vscode_fork;
pub mod windsurf;
pub mod zed;

pub use adapter::*;

/// Build the registry with all supported tools.
pub fn registry() -> ToolRegistry {
    ToolRegistry::new(vec![
        Box::new(CursorTool),
        Box::new(ClaudeTool),
        Box::new(GeminiTool),
        Box::new(WindsurfTool),
        Box::new(AiderTool),
        Box::new(CopilotTool),
        Box::new(CodexTool),
        Box::new(OpenCodeTool),
        Box::new(TraeTool),
        Box::new(ZedTool),
        Box::new(KiroTool),
        Box::new(ContinueTool),
        Box::new(DroidTool),
        Box::new(JunieTool),
        Box::new(AmpTool),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_has_all_tools() {
        let reg = registry();
        let names = reg.tool_names();
        // Must match the number of Box::new(...) entries in registry()
        let expected = [
            "cursor", "claude", "gemini", "windsurf", "aider", "copilot", "codex", "opencode",
            "trae", "zed", "kiro", "continue", "droid", "junie", "amp",
        ];
        assert_eq!(
            names.len(),
            expected.len(),
            "registry has {} tools, expected {}: {:?}",
            names.len(),
            expected.len(),
            names,
        );
    }

    #[test]
    fn test_registry_names_unique() {
        let reg = registry();
        let names: Vec<&str> = reg.all().map(|t| t.name()).collect();
        let mut deduped = names.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(names.len(), deduped.len(), "duplicate tool names found");
    }

    #[test]
    fn test_registry_display_names_not_empty() {
        let reg = registry();
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
        let reg = registry();
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
        let reg = registry();
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
}
