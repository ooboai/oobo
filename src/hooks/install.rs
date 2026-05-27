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
    if let Some(msg) = install_kiro_hooks() {
        installed.push(msg);
    }
    if let Some(msg) = install_continue_hooks() {
        installed.push(msg);
    }
    if let Some(msg) = install_droid_hooks() {
        installed.push(msg);
    }

    // Amp: Uses the ACP (Agent Communication Protocol) for integrations.
    // No hooks.json or settings.json equivalent exists. Lifecycle events
    // would need to go through the ACP wire protocol.
    //
    // Junie: JetBrains beta tool. No documented hook/plugin system yet.
    // Junie imports from .claude/ settings, so Claude hooks may partially
    // cover Junie sessions when both are installed.

    installed
}

// Cursor  --  ~/.cursor/hooks.json

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
            "preToolUse": [
                { "command": "oobo hooks agent pre-tool-use --tool cursor" }
            ],
            "postToolUse": [
                { "command": "oobo hooks agent after-tool-use --tool cursor" }
            ],
            "postToolUseFailure": [
                { "command": "oobo hooks agent tool-use-failure --tool cursor" }
            ],
            "subagentStart": [
                { "command": "oobo hooks agent subagent-start --tool cursor" }
            ],
            "subagentStop": [
                { "command": "oobo hooks agent subagent-stop --tool cursor" }
            ],
            "afterAgentThought": [
                { "command": "oobo hooks agent after-agent-thought --tool cursor" }
            ],
            "afterAgentResponse": [
                { "command": "oobo hooks agent after-agent-response --tool cursor" }
            ],
            "preCompact": [
                { "command": "oobo hooks agent pre-compact --tool cursor" }
            ],
            "stop": [
                { "command": "oobo hooks agent stop --tool cursor" }
            ],
            "sessionEnd": [
                { "command": "oobo hooks agent session-end --tool cursor" }
            ]
        }
    });

    merge_cursor_hooks_file(&path, &oobo_hooks)?;
    Some(format!("Cursor hooks → {}", path.display()))
}

// Claude Code  --  ~/.claude/settings.json

fn install_claude_hooks() -> Option<String> {
    let path = dirs::home_dir()?.join(".claude/settings.json");
    let oobo_hooks = serde_json::json!({
        "hooks": {
            "SessionStart": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent session-start --tool claude"}]
            }],
            "UserPromptSubmit": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent before-submit-prompt --tool claude"}]
            }],
            "PreToolUse": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent pre-tool-use --tool claude"}]
            }],
            "PostToolUse": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent after-tool-use --tool claude"}]
            }],
            "PostToolUseFailure": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent tool-use-failure --tool claude"}]
            }],
            "SubagentStart": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent subagent-start --tool claude"}]
            }],
            "SubagentStop": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent subagent-stop --tool claude"}]
            }],
            "Stop": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent stop --tool claude"}]
            }],
            "SessionEnd": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent session-end --tool claude"}]
            }]
        }
    });

    merge_claude_hooks_file(&path, &oobo_hooks)?;
    Some(format!("Claude Code hooks → {}", path.display()))
}

// Gemini CLI  --  ~/.gemini/settings.json

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

// OpenCode  --  ~/.config/opencode/plugins/oobo.ts

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
            tracing::warn!(path = %parent.display(), %e, "could not create directory");
            return None;
        }
    }
    if let Err(e) = fs::write(&path, content) {
        tracing::warn!(path = %path.display(), %e, "could not write file");
        return None;
    }
    Some(format!("OpenCode plugin → {}", path.display()))
}

// Kiro  --  ~/.kiro/agents/oobo.json (Kiro agent config format)

fn install_kiro_hooks() -> Option<String> {
    let dir = dirs::home_dir()?.join(".kiro/agents");
    let path = dir.join("oobo.json");

    let agent_config = serde_json::json!({
        "name": "oobo",
        "description": "oobo lifecycle hooks for session and tool tracking",
        "hooks": {
            "agentSpawn": [
                { "command": "oobo hooks agent session-start --tool kiro" }
            ],
            "userPromptSubmit": [
                { "command": "oobo hooks agent before-submit-prompt --tool kiro" }
            ],
            "preToolUse": [
                { "command": "oobo hooks agent pre-tool-use --tool kiro" }
            ],
            "postToolUse": [
                { "command": "oobo hooks agent after-tool-use --tool kiro" }
            ],
            "stop": [
                { "command": "oobo hooks agent stop --tool kiro" }
            ]
        }
    });

    if path.exists() {
        if let Ok(existing) = fs::read_to_string(&path) {
            if existing.contains("oobo hooks agent") {
                return Some(format!(
                    "Kiro agent hooks → {} (already installed)",
                    path.display()
                ));
            }
        }
    }

    fs::create_dir_all(&dir).ok()?;
    let json = serde_json::to_string_pretty(&agent_config).ok()?;
    fs::write(&path, json).ok()?;
    Some(format!("Kiro agent hooks → {}", path.display()))
}

// Continue  --  ~/.continue/settings.json (Claude Code-compatible format)

fn install_continue_hooks() -> Option<String> {
    let path = dirs::home_dir()?.join(".continue/settings.json");
    let oobo_hooks = serde_json::json!({
        "hooks": {
            "SessionStart": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent session-start --tool continue"}]
            }],
            "UserPromptSubmit": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent before-submit-prompt --tool continue"}]
            }],
            "PostToolUse": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent after-tool-use --tool continue"}]
            }],
            "PostToolUseFailure": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent tool-use-failure --tool continue"}]
            }],
            "Stop": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent stop --tool continue"}]
            }],
            "SessionEnd": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent session-end --tool continue"}]
            }]
        }
    });

    merge_claude_hooks_file(&path, &oobo_hooks)?;
    Some(format!("Continue hooks → {}", path.display()))
}

// Factory Droid  --  ~/.factory/settings.json (Claude Code-compatible format)

fn install_droid_hooks() -> Option<String> {
    let path = dirs::home_dir()?.join(".factory/settings.json");
    let oobo_hooks = serde_json::json!({
        "hooks": {
            "SessionStart": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent session-start --tool droid"}]
            }],
            "PostToolUse": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent after-tool-use --tool droid"}]
            }],
            "PostToolUseFailure": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent tool-use-failure --tool droid"}]
            }],
            "Stop": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent stop --tool droid"}]
            }],
            "SessionEnd": [{
                "hooks": [{"type": "command", "command": "oobo hooks agent session-end --tool droid"}]
            }]
        }
    });

    merge_claude_hooks_file(&path, &oobo_hooks)?;
    Some(format!("Factory Droid hooks → {}", path.display()))
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

/// Merge oobo hooks into Claude Code's ~/.claude/settings.json.
/// Claude uses `{ "hooks": { "<Event>": [{ "matcher?": "...", "hooks": [{ "type": "command", "command": "..." }] }] } }`.
/// For each event, we upsert oobo's matcher group: remove stale oobo entries then insert fresh ones.
fn merge_claude_hooks_file(path: &Path, oobo_config: &serde_json::Value) -> Option<()> {
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
    if !obj.contains_key("hooks") {
        obj.insert("hooks".to_string(), serde_json::json!({}));
    }
    let hooks_obj = obj.get_mut("hooks")?.as_object_mut()?;
    let oobo_hooks = oobo_config.get("hooks")?.as_object()?;

    for (event, matcher_groups) in oobo_hooks {
        let oobo_arr = matcher_groups.as_array()?;
        if let Some(existing_arr) = hooks_obj.get_mut(event).and_then(|v| v.as_array_mut()) {
            // Remove stale oobo matcher groups (any group whose hooks contain "oobo hooks agent")
            existing_arr.retain(|group| {
                let hooks = group.get("hooks").and_then(|h| h.as_array());
                !hooks.is_some_and(|arr| {
                    arr.iter().any(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .is_some_and(|c| c.contains("oobo hooks agent"))
                    })
                })
            });
            for group in oobo_arr {
                existing_arr.push(group.clone());
            }
        } else {
            hooks_obj.insert(event.clone(), matcher_groups.clone());
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
                        merged.insert(k.clone(), v.clone());
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
        let is_oobo_hook = existing.contains("oobo hooks") || existing.contains("anchor hooks");
        if is_oobo_hook {
            // Overwrite in-place; no backup needed since this is our own hook.
            // Also clean up any stale .pre-anchor backup that may reference itself.
            let stale_backup = hooks_dir.join(format!("{hook_name}.pre-anchor"));
            if stale_backup.exists() {
                let backup_content = fs::read_to_string(&stale_backup).unwrap_or_default();
                if backup_content.contains("oobo hooks") || backup_content.contains("anchor hooks")
                {
                    let _ = fs::remove_file(&stale_backup);
                }
            }
            fs::write(&hook_path, script).map_err(|e| format!("cannot write hook: {e}"))?;
            return Ok(());
        }

        let backup = hooks_dir.join(format!("{hook_name}.pre-anchor"));
        fs::copy(&hook_path, &backup).map_err(|e| format!("cannot backup hook: {e}"))?;

        let chained = format!(
            "{script}\n\n# Chain with original hook\nif [ -x \"$(dirname \"$0\")/{hook_name}.pre-anchor\" ]; then\n  \"$(dirname \"$0\")/{hook_name}.pre-anchor\" \"$@\"\nfi\n"
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

/// Install git hooks for a project.
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

    let post_merge = format!(
        "#!/bin/sh\nmkdir -p {log_dir}\noobo hooks post-merge \"$@\" 2>>{log_dir}/hooks.log || true\n"
    );
    install_git_hook(project_root, "post-merge", &post_merge)?;
    installed.push(format!("post-merge → {}/", hooks_display.display()));

    let post_rewrite = format!(
        "#!/bin/sh\nmkdir -p {log_dir}\noobo hooks post-rewrite \"$@\" 2>>{log_dir}/hooks.log || true\n"
    );
    install_git_hook(project_root, "post-rewrite", &post_rewrite)?;
    installed.push(format!("post-rewrite → {}/", hooks_display.display()));

    Ok(installed)
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
        assert!(hook.contains("pre-anchor"));

        let backup = fs::read_to_string(hooks_dir.join("post-commit.pre-anchor")).unwrap();
        assert!(backup.contains("original"));
    }

    #[test]
    fn test_install_git_hook_overwrites_old_anchor_binary() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();
        let hooks_dir = dir.path().join(".git/hooks");
        fs::create_dir_all(&hooks_dir).unwrap();

        // Old hook using legacy `anchor` binary name
        fs::write(
            hooks_dir.join("post-commit"),
            "#!/bin/sh\nanchor hooks post-commit\n",
        )
        .unwrap();

        install_git_hook(root, "post-commit", "#!/bin/sh\noobo hooks post-commit\n").unwrap();

        let hook = fs::read_to_string(hooks_dir.join("post-commit")).unwrap();
        assert!(hook.contains("oobo hooks"));
        assert!(!hook.contains("pre-anchor"), "must not chain to itself");
        assert!(
            !hooks_dir.join("post-commit.pre-anchor").exists(),
            "must not create a self-referencing backup"
        );
    }

    #[test]
    fn test_install_git_hook_cleans_stale_pre_anchor() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();
        let hooks_dir = dir.path().join(".git/hooks");
        fs::create_dir_all(&hooks_dir).unwrap();

        // Simulate the broken state: hook with oobo AND a self-referencing backup
        fs::write(
            hooks_dir.join("post-commit"),
            "#!/bin/sh\noobo hooks post-commit\nif [ -x post-commit.pre-anchor ]; then post-commit.pre-anchor; fi\n",
        )
        .unwrap();
        fs::write(
            hooks_dir.join("post-commit.pre-anchor"),
            "#!/bin/sh\nanchor hooks post-commit\nif [ -x post-commit.pre-anchor ]; then post-commit.pre-anchor; fi\n",
        )
        .unwrap();

        install_git_hook(root, "post-commit", "#!/bin/sh\noobo hooks post-commit\n").unwrap();

        let hook = fs::read_to_string(hooks_dir.join("post-commit")).unwrap();
        assert!(hook.contains("oobo hooks"));
        assert!(!hook.contains("pre-anchor"), "cleaned hook must not chain");
        assert!(
            !hooks_dir.join("post-commit.pre-anchor").exists(),
            "stale self-referencing backup must be removed"
        );
    }

    #[test]
    fn test_merge_claude_hooks_file_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        let oobo = serde_json::json!({
            "hooks": {
                "SessionStart": [{"hooks": [{"type": "command", "command": "oobo hooks agent session-start --tool claude"}]}],
                "Stop": [{"hooks": [{"type": "command", "command": "oobo hooks agent stop --tool claude"}]}]
            }
        });
        merge_claude_hooks_file(&path, &oobo);

        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let starts = content["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(starts.len(), 1);
        let cmd = starts[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.contains("session-start"));
    }

    #[test]
    fn test_merge_claude_hooks_file_preserves_user_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        let existing = serde_json::json!({
            "hooks": {
                "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "user-script.sh"}]}]
            },
            "other_setting": true
        });
        fs::write(&path, serde_json::to_string_pretty(&existing).unwrap()).unwrap();

        let oobo = serde_json::json!({
            "hooks": {
                "Stop": [{"hooks": [{"type": "command", "command": "oobo hooks agent stop --tool claude"}]}]
            }
        });
        merge_claude_hooks_file(&path, &oobo);

        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["other_setting"], true);
        assert_eq!(content["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(content["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_merge_claude_hooks_file_upgrades_stale_oobo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        let stale = serde_json::json!({
            "hooks": {
                "Stop": [
                    {"hooks": [{"type": "command", "command": "oobo hooks agent stop --tool claude"}]},
                    {"matcher": "Bash", "hooks": [{"type": "command", "command": "user-stop.sh"}]}
                ]
            }
        });
        fs::write(&path, serde_json::to_string_pretty(&stale).unwrap()).unwrap();

        let oobo = serde_json::json!({
            "hooks": {
                "Stop": [{"hooks": [{"type": "command", "command": "oobo hooks agent stop --tool claude"}]}],
                "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "oobo hooks agent before-submit-prompt --tool claude"}]}]
            }
        });
        merge_claude_hooks_file(&path, &oobo);

        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let stops = content["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stops.len(), 2); // user hook + fresh oobo hook
        let user_cmd = stops[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(user_cmd.contains("user-stop.sh"));
        let oobo_cmd = stops[1]["hooks"][0]["command"].as_str().unwrap();
        assert!(oobo_cmd.contains("oobo hooks agent stop"));
        assert!(
            content["hooks"]["UserPromptSubmit"]
                .as_array()
                .unwrap()
                .len()
                == 1
        );
    }
}
