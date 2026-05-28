//! `oobo mcp install` -- configure AI tools to use the oobo MCP server.

use crate::config::Config;
use crate::error::CmdResult;
use std::fs;
use std::path::{Path, PathBuf};

struct ToolTarget {
    name: &'static str,
    config_path: PathBuf,
}

pub fn run(cfg: &Config, tool: Option<&str>, hosted: bool, remove: bool) -> CmdResult {
    let targets = if let Some(name) = tool {
        if let Some(t) = resolve_target(name) { vec![t] } else {
            eprintln!(
                "error: unknown tool '{name}'. Supported: cursor, claude, copilot"
            );
            return Ok(2);
        }
    } else {
        detect_tools()
    };

    if targets.is_empty() {
        eprintln!("No supported AI tools detected. Specify one explicitly:");
        eprintln!("  oobo mcp install cursor");
        eprintln!("  oobo mcp install claude");
        return Ok(1);
    }

    for target in &targets {
        if remove {
            remove_entry(target)?;
        } else {
            install_entry(cfg, target, hosted)?;
        }
    }

    if !remove {
        let has_key = !cfg.server.api_key.is_empty()
            || std::env::var("OOBO_API_KEY").map(|v| !v.is_empty()).unwrap_or(false);

        eprintln!();
        if hosted && !has_key {
            eprintln!("  Note: Set OOBO_API_KEY environment variable for the hosted MCP to authenticate.");
            eprintln!("        Get your key at https://app.oobo.ai/settings/api-keys");
            eprintln!();
        } else if !has_key {
            eprintln!("  Note: Cloud memory tools (recall, get_context, ask) require an API key.");
            eprintln!("        Run: oobo settings set key <KEY>");
            eprintln!("        Or set OOBO_API_KEY environment variable.");
            eprintln!("        Local code search works without a key.");
            eprintln!();
        }
        eprintln!("Done. Restart your AI tool to activate.");
    }

    Ok(0)
}

fn resolve_target(name: &str) -> Option<ToolTarget> {
    let home = dirs::home_dir()?;
    match name.to_lowercase().as_str() {
        "cursor" => Some(ToolTarget {
            name: "cursor",
            config_path: home.join(".cursor").join("mcp.json"),
        }),
        "claude" => Some(ToolTarget {
            name: "claude",
            config_path: home.join(".claude.json"),
        }),
        "copilot" => Some(ToolTarget {
            name: "copilot",
            config_path: home.join(".vscode").join("mcp.json"),
        }),
        _ => None,
    }
}

fn detect_tools() -> Vec<ToolTarget> {
    let mut found = Vec::new();
    let Some(home) = dirs::home_dir() else {
        return found;
    };

    if home.join(".cursor").exists() {
        found.push(ToolTarget {
            name: "cursor",
            config_path: home.join(".cursor").join("mcp.json"),
        });
    }

    if home.join(".claude.json").exists() || home.join(".claude").exists() {
        found.push(ToolTarget {
            name: "claude",
            config_path: home.join(".claude.json"),
        });
    }

    found
}

fn install_entry(_cfg: &Config, target: &ToolTarget, hosted: bool) -> CmdResult {
    let entry = if hosted {
        serde_json::json!({
            "url": "https://agentic.oobo.ai/mcp",
            "headers": { "Authorization": "Bearer ${OOBO_API_KEY}" }
        })
    } else {
        serde_json::json!({
            "command": "oobo",
            "args": ["mcp"]
        })
    };

    let mut config = read_mcp_config(&target.config_path);
    let servers = config
        .as_object_mut()
        .unwrap()
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    servers
        .as_object_mut()
        .unwrap()
        .insert("oobo".to_string(), entry);

    let other_count = servers.as_object().map_or(0, |m| m.len().saturating_sub(1));

    write_mcp_config(&target.config_path, &config)?;

    let preserved = if other_count > 0 {
        format!(" ({other_count} other server{} preserved)", if other_count == 1 { "" } else { "s" })
    } else {
        String::new()
    };

    eprintln!(
        "  {:<8} {}     added \"oobo\"{}",
        target.name,
        target.config_path.display(),
        preserved,
    );

    Ok(0)
}

fn remove_entry(target: &ToolTarget) -> CmdResult {
    if !target.config_path.exists() {
        eprintln!("  {:<8} not configured (skipped)", target.name);
        return Ok(0);
    }

    let mut config = read_mcp_config(&target.config_path);
    if let Some(servers) = config.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
        if servers.remove("oobo").is_some() {
            write_mcp_config(&target.config_path, &config)?;
            eprintln!("  {:<8} removed \"oobo\" from {}", target.name, target.config_path.display());
        } else {
            eprintln!("  {:<8} \"oobo\" not found (skipped)", target.name);
        }
    }

    Ok(0)
}

fn read_mcp_config(path: &Path) -> serde_json::Value {
    if path.exists() {
        let content = fs::read_to_string(path).unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    }
}

fn write_mcp_config(path: &Path, config: &serde_json::Value) -> CmdResult {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            crate::error::CliError::User(format!(
                "Failed to create directory {}: {e}",
                parent.display()
            ))
        })?;
    }

    let json = serde_json::to_string_pretty(config).map_err(|e| {
        crate::error::CliError::User(format!("Failed to serialize config: {e}"))
    })?;

    fs::write(path, format!("{json}\n")).map_err(|e| {
        crate::error::CliError::User(format!(
            "Failed to write {}: {e}",
            path.display()
        ))
    })?;

    Ok(0)
}
