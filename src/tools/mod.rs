use crate::core::tool::ToolRegistry;

mod adapter;

pub mod aider;
pub mod claude;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod gemini;
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
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_has_all_tools() {
        let reg = registry();
        let names = reg.tool_names();
        assert_eq!(names.len(), 10, "expected 10 tools, got {}", names.len());
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
}
