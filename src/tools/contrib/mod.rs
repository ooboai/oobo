//! Community-contributed tool adapters.
//!
//! These adapters are held to a lower coverage bar than first-class integrations
//! (Cursor, Claude Code, Gemini CLI, OpenCode, Codex, Aider, Copilot, Zed,
//! Continue, Factory Droid). They may ship with discovery-only support, missing
//! native token counts, or partial transcript parsing. Enable them by setting
//! `tools.experimental = true` in `~/.oobo/config.toml`.

pub mod amp;
pub mod junie;
pub mod kiro;
pub mod trae;
pub mod windsurf;
