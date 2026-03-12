use std::fs;
use std::path::Path;

/// Install agent lifecycle hooks for all supported tools.
/// Called by `oobo setup`. Merges into existing config files
/// without overwriting user settings.
pub fn install_all_agent_hooks() -> Vec<String> {
    let mut installed = Vec::new();

    if let Some(msg) = install_cursor_hooks() {
        installed.push(msg);
    }
    if let Some(msg) = install_claude_hooks() {
        installed.push(msg);
    }
    if let Some(msg) = install_gemini_hooks() {
        installed.push(msg);
    }
    if let Some(msg) = install_opencode_hooks() {
        installed.push(msg);
    }

    installed
}

// Cursor — ~/.cursor/hooks.json

fn install_cursor_hooks() -> Option<String> {
    let path = dirs::home_dir()?.join(".cursor/hooks.json");
    let oobo_hooks = serde_json::json!({
        "version": 1,
        "hooks": {
            "sessionStart": [
                { "command": "oobo hooks agent session-start --tool cursor" }
            ],
            "beforeSubmitPrompt": [
                { "command": "oobo hooks agent before-submit-prompt --tool cursor" }
            ],
            "stop": [
                { "command": "oobo hooks agent stop --tool cursor" }
            ],
            "subagentStop": [
                { "command": "oobo hooks agent subagent-stop --tool cursor" }
            ],
            "sessionEnd": [
                { "command": "oobo hooks agent session-end --tool cursor" }
            ]
        }
    });

    merge_cursor_hooks_file(&path, &oobo_hooks)?;
    Some(format!("Cursor hooks → {}", path.display()))
}

// Claude Code — ~/.claude/settings.json

fn install_claude_hooks() -> Option<String> {
    let path = dirs::home_dir()?.join(".claude/settings.json");
    let oobo_hooks = serde_json::json!({
        "hooks": {
            "SessionStart": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent session-start --tool claude"}]
            }],
            "Stop": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent stop --tool claude"}]
            }],
            "SessionEnd": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent session-end --tool claude"}]
            }]
        }
    });

    merge_json_file(&path, &oobo_hooks, &["hooks"])?;
    Some(format!("Claude Code hooks → {}", path.display()))
}

// Gemini CLI — ~/.gemini/settings.json

fn install_gemini_hooks() -> Option<String> {
    let path = dirs::home_dir()?.join(".gemini/settings.json");
    let oobo_hooks = serde_json::json!({
        "hooks": {
            "SessionStart": [{
                "type": "command",
                "command": "oobo hooks agent session-start --tool gemini"
            }],
            "SessionEnd": [{
                "type": "command",
                "command": "oobo hooks agent session-end --tool gemini"
            }],
            "AfterAgent": [{
                "type": "command",
                "command": "oobo hooks agent stop --tool gemini"
            }]
        }
    });

    merge_json_file(&path, &oobo_hooks, &["hooks"])?;
    Some(format!("Gemini CLI hooks → {}", path.display()))
}

// OpenCode — ~/.config/opencode/plugins/oobo.ts

fn install_opencode_hooks() -> Option<String> {
    let path = dirs::config_dir()?.join("opencode/plugins/oobo.ts");
    let content = r#"export default async ({ client }) => ({
  event: async ({ event }) => {
    const { execSync } = require("child_process");
    const input = JSON.stringify(event);
    try {
      if (event.type === "session.created")
        execSync("oobo hooks agent session-start --tool opencode", { input, encoding: "utf-8" });
      if (event.type === "session.deleted")
        execSync("oobo hooks agent session-end --tool opencode", { input, encoding: "utf-8" });
    } catch (_) {}
  }
});
"#;

    if path.exists() {
        if let Ok(existing) = fs::read_to_string(&path) {
            if existing.contains("oobo hooks agent") {
                return Some(format!(
                    "OpenCode plugin → {} (already installed)",
                    path.display()
                ));
            }
        }
    }

    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            eprintln!("oobo: warning: could not create {}: {e}", parent.display());
            return None;
        }
    }
    if let Err(e) = fs::write(&path, content) {
        eprintln!("oobo: warning: could not write {}: {e}", path.display());
        return None;
    }
    Some(format!("OpenCode plugin → {}", path.display()))
}

// Helpers

/// Merge oobo hooks into Cursor's hooks.json.
/// Cursor uses `{ "version": 1, "hooks": { "<event>": [{ "command": "..." }] } }`.
/// Each event maps to an array of handler objects. We append oobo handlers
/// without duplicating or clobbering existing ones.
/// Also migrates the legacy `{ "agent": { ... } }` format if found.
fn merge_cursor_hooks_file(path: &Path, oobo_config: &serde_json::Value) -> Option<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok()?;
    }

    let mut existing: serde_json::Value = if path.exists() {
        let content = fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let obj = existing.as_object_mut()?;

    // Remove legacy "agent" key (old broken format)
    obj.remove("agent");

    obj.insert("version".to_string(), serde_json::json!(1));

    let oobo_hooks = oobo_config.get("hooks")?.as_object()?;

    if !obj.contains_key("hooks") {
        obj.insert("hooks".to_string(), serde_json::json!({}));
    }
    let hooks_obj = obj.get_mut("hooks")?.as_object_mut()?;

    for (event, handlers) in oobo_hooks {
        let oobo_arr = handlers.as_array()?;
        if let Some(existing_arr) = hooks_obj.get_mut(event).and_then(|v| v.as_array_mut()) {
            // Remove any existing oobo commands (handles upgrades, e.g. adding --tool flag)
            existing_arr.retain(|h| {
                let cmd = h.get("command").and_then(|c| c.as_str()).unwrap_or("");
                !cmd.contains("oobo hooks agent")
            });
            for handler in oobo_arr {
                existing_arr.push(handler.clone());
            }
        } else {
            hooks_obj.insert(event.clone(), handlers.clone());
        }
    }

    let json = serde_json::to_string_pretty(&existing).ok()?;
    fs::write(path, json).ok()?;

    Some(())
}

/// Merge oobo's JSON keys into an existing JSON file without clobbering.
/// Creates the file and parent dirs if they don't exist.
/// Only merges top-level keys listed in `merge_keys`.
fn merge_json_file(
    path: &Path,
    oobo_config: &serde_json::Value,
    merge_keys: &[&str],
) -> Option<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok()?;
    }

    let mut existing: serde_json::Value = if path.exists() {
        let content = fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let existing_obj = existing.as_object_mut()?;
    let oobo_obj = oobo_config.as_object()?;

    for key in merge_keys {
        if let Some(oobo_val) = oobo_obj.get(*key) {
            if existing_obj.contains_key(*key) {
                let existing_section = existing_obj.get(*key).unwrap();
                if let (Some(existing_inner), Some(oobo_inner)) =
                    (existing_section.as_object(), oobo_val.as_object())
                {
                    let mut merged = existing_inner.clone();
                    for (k, v) in oobo_inner {
                        if !merged.contains_key(k) {
                            merged.insert(k.clone(), v.clone());
                        }
                    }
                    existing_obj.insert(key.to_string(), serde_json::Value::Object(merged));
                }
            } else {
                existing_obj.insert(key.to_string(), oobo_val.clone());
            }
        }
    }

    let json = serde_json::to_string_pretty(&existing).ok()?;
    fs::write(path, json).ok()?;

    Some(())
}

/// Install a per-repo git hook. Chains with existing hooks if present.
pub fn install_git_hook(project_root: &str, hook_name: &str, script: &str) -> Result<(), String> {
    let hooks_dir = crate::git::detect::resolve_git_dir(project_root).join("hooks");
    fs::create_dir_all(&hooks_dir)
        .map_err(|e| format!("cannot create {}: {e}", hooks_dir.display()))?;

    let hook_path = hooks_dir.join(hook_name);

    if hook_path.exists() {
        let existing = fs::read_to_string(&hook_path).unwrap_or_default();
        if existing.contains("oobo") {
            return Ok(());
        }

        let backup = hooks_dir.join(format!("{hook_name}.pre-oobo"));
        fs::copy(&hook_path, &backup).map_err(|e| format!("cannot backup hook: {e}"))?;

        let chained = format!(
            "{script}\n\n# Chain with original hook\nif [ -x \"$(dirname \"$0\")/{hook_name}.pre-oobo\" ]; then\n  \"$(dirname \"$0\")/{hook_name}.pre-oobo\" \"$@\"\nfi\n"
        );
        fs::write(&hook_path, chained).map_err(|e| format!("cannot write hook: {e}"))?;
    } else {
        fs::write(&hook_path, script).map_err(|e| format!("cannot write hook: {e}"))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        fs::set_permissions(&hook_path, perms).map_err(|e| format!("cannot chmod hook: {e}"))?;
    }

    Ok(())
}

/// Install post-commit and pre-push hooks for a project.
pub fn install_project_hooks(project_root: &str) -> Result<Vec<String>, String> {
    let mut installed = Vec::new();

    let log_dir = "\"${XDG_DATA_HOME:-$HOME/.local/share}/oobo/logs\"";
    let post_commit = format!(
        "#!/bin/sh\nmkdir -p {log_dir}\noobo hooks post-commit \"$@\" 2>>{log_dir}/hooks.log || true\n"
    );
    install_git_hook(project_root, "post-commit", &post_commit)?;
    let hooks_display = crate::git::detect::resolve_git_dir(project_root).join("hooks");
    installed.push(format!("post-commit → {}/", hooks_display.display()));

    let pre_push = format!(
        "#!/bin/sh\nmkdir -p {log_dir}\noobo hooks pre-push \"$@\" 2>>{log_dir}/hooks.log || true\n"
    );
    install_git_hook(project_root, "pre-push", &pre_push)?;
    installed.push(format!("pre-push → {}/", hooks_display.display()));

    Ok(installed)
}

pub fn check_installed_hooks() -> Vec<String> {
    let mut found = Vec::new();
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return found,
    };

    let cursor_hooks = home.join(".cursor/hooks.json");
    if cursor_hooks.exists() {
        if let Ok(c) = fs::read_to_string(&cursor_hooks) {
            if c.contains("oobo") {
                found.push("cursor".into());
            }
        }
    }

    let claude_settings = home.join(".claude/settings.json");
    if claude_settings.exists() {
        if let Ok(c) = fs::read_to_string(&claude_settings) {
            if c.contains("oobo") {
                found.push("claude".into());
            }
        }
    }

    let gemini_settings = home.join(".gemini/settings.json");
    if gemini_settings.exists() {
        if let Ok(c) = fs::read_to_string(&gemini_settings) {
            if c.contains("oobo") {
                found.push("gemini".into());
            }
        }
    }

    if let Some(config_dir) = dirs::config_dir() {
        let opencode_plugin = config_dir.join("opencode/plugins/oobo.ts");
        if opencode_plugin.exists() {
            found.push("opencode".into());
        }
    }

    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_json_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.json");

        let oobo = serde_json::json!({"hooks": {"start": "oobo start"}});
        merge_json_file(&path, &oobo, &["hooks"]);

        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["hooks"]["start"], "oobo start");

        let existing = serde_json::json!({"hooks": {"start": "oobo start"}, "other": true});
        fs::write(&path, serde_json::to_string(&existing).unwrap()).unwrap();

        let oobo2 = serde_json::json!({"hooks": {"end": "oobo end"}});
        merge_json_file(&path, &oobo2, &["hooks"]);

        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["hooks"]["start"], "oobo start");
        assert_eq!(content["hooks"]["end"], "oobo end");
        assert_eq!(content["other"], true);
    }

    #[test]
    fn test_install_git_hook_new() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();

        install_git_hook(root, "post-commit", "#!/bin/sh\noobo test\n").unwrap();

        let hook = fs::read_to_string(dir.path().join(".git/hooks/post-commit")).unwrap();
        assert!(hook.contains("oobo test"));
    }

    #[test]
    fn test_install_git_hook_chains() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();
        let hooks_dir = dir.path().join(".git/hooks");
        fs::create_dir_all(&hooks_dir).unwrap();

        fs::write(hooks_dir.join("post-commit"), "#!/bin/sh\noriginal\n").unwrap();

        install_git_hook(root, "post-commit", "#!/bin/sh\noobo test\n").unwrap();

        let hook = fs::read_to_string(hooks_dir.join("post-commit")).unwrap();
        assert!(hook.contains("oobo test"));
        assert!(hook.contains("pre-oobo"));

        let backup = fs::read_to_string(hooks_dir.join("post-commit.pre-oobo")).unwrap();
        assert!(backup.contains("original"));
    }
}
