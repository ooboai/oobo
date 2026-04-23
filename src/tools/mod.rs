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
    registry_with(
        crate::config::Config::load_or_default()
            .tools
            .experimental,
    )
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
        assert_eq!(names.len(), 15, "experimental registry should have 15 tools, got {names:?}");
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
        let names: Vec<&str> = reg.all().map(|t| t.name()).collect();
        let mut deduped = names.clone();
        deduped.sort();
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
}
