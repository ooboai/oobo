//! `oobo settings` — declarative KV config.
//!
//! Grammar: `oobo settings [scope] [verb] <key> [value]`
//! - scope (optional, default `default`): `default` | `project`
//! - verb  (optional, default `get`):   `set` | `unset`
//! - key/value: positional, no flags.

use crate::cli::OutputMode;
use crate::config::Config;
use crate::error::{CliError, CmdResult};

const RESERVED_SCOPES: &[&str] = &["default", "project"];
const RESERVED_VERBS: &[&str] = &["set", "unset"];

const VALID_KEYS: &[&str] = &[
    "key",
    "api_url",
    "remote",
    "transparency",
    "tools.experimental",
    "setup.scan_roots",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Default,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Get,
    Set,
    Unset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub scope: Option<Scope>,
    pub verb: Verb,
    pub key: Option<String>,
    pub value: Option<String>,
}

/// Parse the free-form positional arguments after `oobo settings`.
///
/// Grammar: [scope] [verb] <key> [value]
/// Reserved scope/verb words are recognized ONLY in positions 1–2; after
/// that, any token is treated as key or value.
pub fn parse_args(args: &[String]) -> Result<Parsed, CliError> {
    let mut idx = 0;
    let mut scope: Option<Scope> = None;
    let mut verb: Option<Verb> = None;

    // Position 1: scope, verb, or key
    if let Some(a) = args.first() {
        if let Some(s) = match_scope(a) {
            scope = Some(s);
            idx += 1;
        } else if let Some(v) = match_verb(a) {
            verb = Some(v);
            idx += 1;
        }
    }

    // Position 2: verb (if scope was consumed), or key
    if idx == 1 && verb.is_none() {
        if let Some(a) = args.get(1) {
            if let Some(v) = match_verb(a) {
                verb = Some(v);
                idx += 1;
            } else if match_scope(a).is_some() {
                return Err(format!("unexpected scope in position 2: '{a}'").into());
            }
        }
    }

    let verb = verb.unwrap_or(Verb::Get);

    let key = args.get(idx).cloned();
    let value = args.get(idx + 1).cloned();

    if args.len() > idx + 2 {
        return Err(CliError::User(format!(
            "too many arguments (got {}): oobo settings [scope] [verb] <key> [value]",
            args.len()
        )));
    }

    Ok(Parsed {
        scope,
        verb,
        key,
        value,
    })
}

fn match_scope(token: &str) -> Option<Scope> {
    if !RESERVED_SCOPES.contains(&token) {
        return None;
    }
    match token {
        "default" => Some(Scope::Default),
        "project" => Some(Scope::Project),
        _ => None,
    }
}

fn match_verb(token: &str) -> Option<Verb> {
    if !RESERVED_VERBS.contains(&token) {
        return None;
    }
    match token {
        "set" => Some(Verb::Set),
        "unset" => Some(Verb::Unset),
        _ => None,
    }
}

/// Entry point invoked by `src/cli.rs`.
pub fn run(cfg: &Config, args: &[String], mode: OutputMode) -> CmdResult {
    let parsed = match parse_args(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return Ok(2);
        }
    };

    match parsed.verb {
        Verb::Get => Ok(run_get(cfg, parsed.scope, parsed.key.as_deref(), mode)),
        Verb::Set => run_set(
            cfg,
            parsed.scope,
            parsed.key.as_deref(),
            parsed.value.as_deref(),
            mode,
        ),
        Verb::Unset => run_unset(cfg, parsed.scope, parsed.key.as_deref(), mode),
    }
}

// ── GET ─────────────────────────────────────────────────────────────────────

fn run_get(cfg: &Config, scope: Option<Scope>, key: Option<&str>, mode: OutputMode) -> i32 {
    match (scope, key) {
        (None, None) => show_effective(cfg, mode),
        (Some(Scope::Default), None) => show_default(cfg, mode),
        (Some(Scope::Project), None) => show_project(cfg, mode),
        (None, Some(k)) => show_key_effective(cfg, k, mode),
        (Some(Scope::Default), Some(k)) => show_key_default(cfg, k, mode),
        (Some(Scope::Project), Some(k)) => show_key_project(cfg, k, mode),
    }
}

fn show_effective(cfg: &Config, mode: OutputMode) -> i32 {
    let project_settings = current_project_settings(cfg);
    let rows: Vec<(String, String, String)> = VALID_KEYS
        .iter()
        .map(|k| {
            let (source, value) = effective_value(cfg, k, project_settings.as_ref());
            (k.to_string(), source, value)
        })
        .collect();
    print_rows(&rows, mode);
    if matches!(mode, OutputMode::Tui) {
        println!();
        println!("  key              API key for remote operations (search, delta)");
        println!("  api_url          API endpoint URL (default: https://api.oobo.ai)");
        println!("  remote           Git remote for anchor branch (default: origin)");
        println!("  transparency     sync transcripts to anchors branch (on/off)");
        println!("  tools.experimental  enable experimental tool adapters (on/off)");
        println!("  setup.scan_roots    comma-separated paths for project scanning (default: ~)");
        println!();
        println!("  set:   oobo settings [default|project] set <key> <value>");
        println!("  unset: oobo settings [default|project] unset <key>");
    }
    0
}

fn show_default(cfg: &Config, mode: OutputMode) -> i32 {
    let rows: Vec<(String, String, String)> = VALID_KEYS
        .iter()
        .map(|k| {
            let v = read_default(cfg, k).unwrap_or_else(|| "(unset)".to_string());
            (k.to_string(), "default".to_string(), mask_if_secret(k, &v))
        })
        .collect();
    print_rows(&rows, mode);
    0
}

fn show_project(cfg: &Config, mode: OutputMode) -> i32 {
    let ps = if let Some(p) = current_project_settings(cfg) {
        p
    } else {
        eprintln!("error: 'project' scope requires being inside a git repo.");
        return 1;
    };
    let overrides: Vec<(String, String, String)> = VALID_KEYS
        .iter()
        .filter_map(|k| {
            read_project(&ps, k)
                .map(|v| (k.to_string(), "project".to_string(), mask_if_secret(k, &v)))
        })
        .collect();
    if overrides.is_empty() {
        println!("no project overrides set. showing defaults:");
        println!("  run: oobo settings default");
        return 0;
    }
    print_rows(&overrides, mode);
    0
}

fn show_key_effective(cfg: &Config, key: &str, mode: OutputMode) -> i32 {
    if let Err(e) = validate_key(key) {
        eprintln!("error: {e}");
        return 2;
    }
    let project_settings = current_project_settings(cfg);
    let (source, value) = effective_value(cfg, key, project_settings.as_ref());
    print_single(key, &source, &value, mode);
    0
}

fn show_key_default(cfg: &Config, key: &str, mode: OutputMode) -> i32 {
    if let Err(e) = validate_key(key) {
        eprintln!("error: {e}");
        return 2;
    }
    let raw = read_default(cfg, key).unwrap_or_else(|| "(unset)".to_string());
    print_single(key, "default", &mask_if_secret(key, &raw), mode);
    0
}

fn show_key_project(cfg: &Config, key: &str, mode: OutputMode) -> i32 {
    if let Err(e) = validate_key(key) {
        eprintln!("error: {e}");
        return 2;
    }
    let ps = if let Some(p) = current_project_settings(cfg) {
        p
    } else {
        eprintln!("error: 'project' scope requires being inside a git repo.");
        return 1;
    };
    if let Some(v) = read_project(&ps, key) {
        print_single(key, "project", &mask_if_secret(key, &v), mode);
    } else {
        let def = read_default(cfg, key).unwrap_or_else(|| "(unset)".to_string());
        println!(
            "{key}   (no project override) falling back to default: {}",
            mask_if_secret(key, &def)
        );
    }
    0
}

// ── SET ─────────────────────────────────────────────────────────────────────

fn run_set(
    cfg: &Config,
    scope: Option<Scope>,
    key: Option<&str>,
    value: Option<&str>,
    mode: OutputMode,
) -> CmdResult {
    let key = if let Some(k) = key {
        k
    } else {
        eprintln!("error: 'set' requires a key: oobo settings [scope] set <key> <value>");
        return Ok(2);
    };
    let value = if let Some(v) = value {
        v
    } else {
        eprintln!("error: 'set' requires a value: oobo settings [scope] set <key> <value>");
        return Ok(2);
    };
    if let Err(e) = validate_key(key) {
        eprintln!("error: {e}");
        return Ok(2);
    }

    match scope.unwrap_or(Scope::Default) {
        Scope::Default => {
            if let Err(e) = validate_value(key, value) {
                eprintln!("error: {e}");
                return Ok(2);
            }
            let mut updated = cfg.clone();
            write_default(&mut updated, key, value);
            updated.save()?;
            match mode {
                OutputMode::Agent => println!("set default {key} {}", mask_if_secret(key, value)),
                OutputMode::Json => {
                    let json = serde_json::json!({
                        "action": "set",
                        "scope": "default",
                        "key": key,
                        "value": mask_if_secret(key, value),
                    });
                    crate::utils::print_json(&json);
                }
                OutputMode::Tui => {
                    println!("set default: {key} = {}", mask_if_secret(key, value));
                }
            }
        }
        Scope::Project => {
            let (project_root, project_id) = if let Some(context) = current_project_context(cfg) {
                context
            } else {
                eprintln!("error: 'project' scope requires being inside a git repo.");
                return Ok(1);
            };
            if let Err(e) = validate_project_key(key) {
                eprintln!("error: {e}");
                return Ok(2);
            }
            if let Err(e) = validate_value(key, value) {
                eprintln!("error: {e}");
                return Ok(2);
            }
            write_project_setting(&project_root, &project_id, key, value)?;
            let project_label = project_label(&project_root, &project_id);
            match mode {
                OutputMode::Agent => {
                    println!(
                        "set project {project_label} {key} {}",
                        mask_if_secret(key, value)
                    );
                }
                OutputMode::Json => {
                    let json = serde_json::json!({
                        "action": "set",
                        "scope": "project",
                        "project": project_id,
                        "key": key,
                        "value": mask_if_secret(key, value),
                    });
                    crate::utils::print_json(&json);
                }
                OutputMode::Tui => {
                    println!(
                        "set project ({project_label}): {key} = {}",
                        mask_if_secret(key, value)
                    );
                }
            }
        }
    }

    Ok(0)
}

// ── UNSET ───────────────────────────────────────────────────────────────────

fn run_unset(cfg: &Config, scope: Option<Scope>, key: Option<&str>, mode: OutputMode) -> CmdResult {
    let key = if let Some(k) = key {
        k
    } else {
        eprintln!("error: 'unset' requires a key: oobo settings [scope] unset <key>");
        return Ok(2);
    };
    if let Err(e) = validate_key(key) {
        eprintln!("error: {e}");
        return Ok(2);
    }

    match scope.unwrap_or(Scope::Default) {
        Scope::Default => {
            let mut updated = cfg.clone();
            unset_default(&mut updated, key);
            updated.save()?;
            match mode {
                OutputMode::Agent => println!("unset default {key}"),
                OutputMode::Json => {
                    let json = serde_json::json!({
                        "action": "unset",
                        "scope": "default",
                        "key": key,
                    });
                    crate::utils::print_json(&json);
                }
                OutputMode::Tui => println!("unset default: {key}"),
            }
        }
        Scope::Project => {
            let (project_root, project_id) = if let Some(context) = current_project_context(cfg) {
                context
            } else {
                eprintln!("error: 'project' scope requires being inside a git repo.");
                return Ok(1);
            };
            if let Err(e) = validate_project_key(key) {
                eprintln!("error: {e}");
                return Ok(2);
            }
            let had = current_project_settings(cfg)
                .and_then(|settings| read_project(&settings, key))
                .is_some();
            unset_project_setting(&project_root, &project_id, key)?;
            if !had {
                println!("no project override for '{key}' to unset.");
                return Ok(0);
            }
            let def = read_default(cfg, key).unwrap_or_else(|| "(unset)".to_string());
            let project_label = project_label(&project_root, &project_id);
            match mode {
                OutputMode::Agent => println!("unset project {project_label} {key}"),
                OutputMode::Json => {
                    let json = serde_json::json!({
                        "action": "unset",
                        "scope": "project",
                        "project": project_id,
                        "key": key,
                    });
                    crate::utils::print_json(&json);
                }
                OutputMode::Tui => {
                    println!(
                        "unset project ({project_label}): {key}. falling back to default: {}",
                        mask_if_secret(key, &def)
                    );
                }
            }
        }
    }

    Ok(0)
}

// ── Key I/O ─────────────────────────────────────────────────────────────────

fn validate_key(key: &str) -> Result<(), CliError> {
    if VALID_KEYS.contains(&key) {
        return Ok(());
    }
    Err(CliError::Config(format!(
        "unknown key '{key}'. valid keys: {}",
        VALID_KEYS.join(", ")
    )))
}

fn validate_value(key: &str, value: &str) -> Result<(), CliError> {
    match key {
        "api_url" => {
            if value.starts_with("http://") || value.starts_with("https://") {
                Ok(())
            } else {
                Err(CliError::Config(format!(
                    "invalid value for 'api_url': expected http(s) URL, got '{value}'"
                )))
            }
        }
        "remote" => {
            if !value.trim().is_empty() && !value.chars().any(char::is_whitespace) {
                Ok(())
            } else {
                Err(CliError::Config(format!(
                    "invalid value for 'remote': expected Git remote name or URL, got '{value}'"
                )))
            }
        }
        "transparency" => match value {
            "on" | "off" => Ok(()),
            _ => Err(CliError::Config(format!(
                "invalid value for 'transparency': expected 'on' or 'off', got '{value}'"
            ))),
        },
        "tools.experimental" => match value {
            "on" | "off" | "true" | "false" => Ok(()),
            _ => Err(CliError::Config(format!(
                "invalid value for 'tools.experimental': expected 'on' or 'off', got '{value}'"
            ))),
        },
        _ => Ok(()),
    }
}

fn validate_project_key(key: &str) -> Result<(), CliError> {
    match key {
        "key" | "api_url" | "remote" | "transparency" => Ok(()),
        "tools.experimental" | "setup.scan_roots" => Err(CliError::Config(format!(
            "'{key}' is a default-only setting; use `oobo settings default set {key} ...`"
        ))),
        _ => validate_key(key),
    }
}

fn read_default(cfg: &Config, key: &str) -> Option<String> {
    match key {
        "key" => {
            let v = &cfg.server.api_key;
            if v.is_empty() {
                None
            } else {
                Some(v.clone())
            }
        }
        "api_url" => Some(cfg.server.url.clone()),
        "remote" => {
            let v = &cfg.anchors.remote;
            if v.is_empty() {
                Some(crate::config::DEFAULT_ANCHOR_REMOTE.to_string())
            } else {
                Some(v.clone())
            }
        }
        "transparency" => Some(cfg.privacy.transparency.clone()),
        "tools.experimental" => Some(if cfg.tools.experimental { "on" } else { "off" }.to_string()),
        "setup.scan_roots" => Some(cfg.setup.scan_roots.join(",")),
        _ => None,
    }
}

fn write_default(cfg: &mut Config, key: &str, value: &str) {
    match key {
        "key" => cfg.server.api_key = value.to_string(),
        "api_url" => cfg.server.url = value.to_string(),
        "remote" => cfg.anchors.remote = value.to_string(),
        "transparency" => cfg.privacy.transparency = value.to_string(),
        "tools.experimental" => {
            cfg.tools.experimental = matches!(value, "on" | "true");
        }
        "setup.scan_roots" => {
            cfg.setup.scan_roots = value.split(',').map(|s| s.trim().to_string()).collect();
        }
        _ => {}
    }
}

fn unset_default(cfg: &mut Config, key: &str) {
    match key {
        "key" => cfg.server.api_key.clear(),
        "api_url" => cfg.server.url = crate::config::DEFAULT_SERVER_URL.to_string(),
        "remote" => cfg.anchors.remote.clear(),
        "transparency" => cfg.privacy.transparency = "on".to_string(),
        "tools.experimental" => cfg.tools.experimental = false,
        "setup.scan_roots" => cfg.setup.scan_roots = vec!["~".to_string()],
        _ => {}
    }
}

#[derive(Default)]
struct ProjectSettings {
    api_key: Option<String>,
    api_url: Option<String>,
    remote: Option<String>,
    transparency: Option<String>,
}

fn read_project(ps: &ProjectSettings, key: &str) -> Option<String> {
    match key {
        "key" => ps.api_key.clone().filter(|s| !s.is_empty()),
        "api_url" => ps.api_url.clone().filter(|s| !s.is_empty()),
        "remote" => ps.remote.clone().filter(|s| !s.is_empty()),
        "transparency" => ps.transparency.clone().filter(|s| !s.is_empty()),
        _ => None,
    }
}

fn write_project_setting(
    project_root: &str,
    project_id: &str,
    key: &str,
    value: &str,
) -> Result<(), CliError> {
    let mut cfg = load_project_config_for_write(project_root, project_id)?;
    match key {
        "key" => cfg.server.api_key = value.to_string(),
        "api_url" => cfg.server.url = value.to_string(),
        "remote" => cfg.anchors.remote = value.to_string(),
        "transparency" => cfg.privacy.transparency = Some(value.to_string()),
        _ => return Ok(()),
    }
    cfg.save(project_root).map_err(CliError::Config)
}

fn unset_project_setting(project_root: &str, project_id: &str, key: &str) -> Result<(), CliError> {
    let mut cfg = load_project_config_for_write(project_root, project_id)?;
    match key {
        "key" => cfg.server.api_key.clear(),
        "api_url" => cfg.server.url.clear(),
        "remote" => cfg.anchors.remote.clear(),
        "transparency" => cfg.privacy.transparency = None,
        _ => return Ok(()),
    }
    cfg.save(project_root).map_err(CliError::Config)
}

fn load_project_config_for_write(
    project_root: &str,
    project_id: &str,
) -> Result<crate::project_config::ProjectConfig, CliError> {
    let mut cfg = crate::project_config::ProjectConfig::load(project_root)?
        .unwrap_or_else(|| crate::project_config::ProjectConfig::for_project(project_id));
    if cfg.project.id.is_empty() {
        cfg.project.id = project_id.to_string();
    }
    Ok(cfg)
}

// ── Effective merge ─────────────────────────────────────────────────────────

fn effective_value(
    cfg: &Config,
    key: &str,
    project_settings: Option<&ProjectSettings>,
) -> (String, String) {
    if let Some(ps) = project_settings {
        if let Some(v) = read_project(ps, key) {
            return ("project".to_string(), mask_if_secret(key, &v));
        }
    }
    let v = read_default(cfg, key).unwrap_or_else(|| "(unset)".to_string());
    ("default".to_string(), mask_if_secret(key, &v))
}

// ── Project context ─────────────────────────────────────────────────────────

fn current_project_context(cfg: &Config) -> Option<(String, String)> {
    let root = crate::git::proxy::project_root(cfg)?;
    let id = crate::project::id_for_root(&root);
    Some((root, id))
}

fn project_label(project_root: &str, fallback_id: &str) -> String {
    std::path::Path::new(project_root)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback_id)
        .to_string()
}

fn current_project_settings(cfg: &Config) -> Option<ProjectSettings> {
    let (root, _id) = current_project_context(cfg)?;
    let mut settings = ProjectSettings::default();
    if let Ok(Some(project_cfg)) = crate::project_config::ProjectConfig::load(&root) {
        if !project_cfg.server.api_key.is_empty() {
            settings.api_key = Some(project_cfg.server.api_key);
        }
        if !project_cfg.server.url.is_empty() {
            settings.api_url = Some(project_cfg.server.url);
        }
        if !project_cfg.anchors.remote.is_empty() {
            settings.remote = Some(project_cfg.anchors.remote);
        }
        if let Some(transparency) = project_cfg.privacy.transparency {
            if !transparency.is_empty() {
                settings.transparency = Some(transparency);
            }
        }
    }
    Some(settings)
}

// ── Secret masking ──────────────────────────────────────────────────────────

fn mask_if_secret(key: &str, value: &str) -> String {
    if key == "key" && !value.is_empty() && value != "(unset)" {
        if value.len() <= 8 {
            return "••••••••".to_string();
        }
        let tail = &value[value.len() - 4..];
        return format!("sk_**********{tail}");
    }
    value.to_string()
}

// ── Output helpers ──────────────────────────────────────────────────────────

fn print_rows(rows: &[(String, String, String)], mode: OutputMode) {
    match mode {
        OutputMode::Json => {
            let mut map = serde_json::Map::new();
            for (k, s, v) in rows {
                map.insert(k.clone(), serde_json::json!({ "source": s, "value": v }));
            }
            let json = serde_json::json!({ "effective": map });
            crate::utils::print_json(&json);
        }
        OutputMode::Agent => {
            for (k, s, v) in rows {
                println!("{k} {s} {v}");
            }
        }
        OutputMode::Tui => {
            println!("{:<22} {:<10} value", "key", "source");
            println!("{:<22} {:<10} ─────", "────", "──────");
            for (k, s, v) in rows {
                println!("{k:<22} {s:<10} {v}");
            }
        }
    }
}

fn print_single(key: &str, source: &str, value: &str, mode: OutputMode) {
    match mode {
        OutputMode::Json => {
            let json = serde_json::json!({
                key: { "source": source, "value": value }
            });
            crate::utils::print_json(&json);
        }
        OutputMode::Agent | OutputMode::Tui => {
            println!("{key}   {source}   {value}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bare() {
        let p = parse_args(&[]).unwrap();
        assert_eq!(p.scope, None);
        assert_eq!(p.verb, Verb::Get);
        assert_eq!(p.key, None);
    }

    #[test]
    fn test_parse_scope_only() {
        let p = parse_args(&["default".to_string()]).unwrap();
        assert_eq!(p.scope, Some(Scope::Default));
        assert_eq!(p.verb, Verb::Get);
    }

    #[test]
    fn test_parse_verb_only() {
        let p = parse_args(&["set".to_string(), "key".to_string(), "v".to_string()]).unwrap();
        assert_eq!(p.scope, None);
        assert_eq!(p.verb, Verb::Set);
        assert_eq!(p.key.as_deref(), Some("key"));
        assert_eq!(p.value.as_deref(), Some("v"));
    }

    #[test]
    fn test_parse_scope_verb() {
        let p = parse_args(&[
            "project".to_string(),
            "set".to_string(),
            "remote".to_string(),
            "oobo".to_string(),
        ])
        .unwrap();
        assert_eq!(p.scope, Some(Scope::Project));
        assert_eq!(p.verb, Verb::Set);
        assert_eq!(p.key.as_deref(), Some("remote"));
    }

    #[test]
    fn test_parse_key_only() {
        let p = parse_args(&["remote".to_string()]).unwrap();
        assert_eq!(p.scope, None);
        assert_eq!(p.verb, Verb::Get);
        assert_eq!(p.key.as_deref(), Some("remote"));
    }

    #[test]
    fn test_parse_scope_key_implicit_get() {
        let p = parse_args(&["project".to_string(), "remote".to_string()]).unwrap();
        assert_eq!(p.scope, Some(Scope::Project));
        assert_eq!(p.verb, Verb::Get);
        assert_eq!(p.key.as_deref(), Some("remote"));
    }

    #[test]
    fn test_parse_unset() {
        let p = parse_args(&[
            "project".to_string(),
            "unset".to_string(),
            "remote".to_string(),
        ])
        .unwrap();
        assert_eq!(p.scope, Some(Scope::Project));
        assert_eq!(p.verb, Verb::Unset);
        assert_eq!(p.key.as_deref(), Some("remote"));
    }

    #[test]
    fn test_parse_rejects_too_many_args() {
        let r = parse_args(&[
            "set".to_string(),
            "key".to_string(),
            "v".to_string(),
            "extra".to_string(),
        ]);
        assert!(r.is_err());
    }

    #[test]
    fn test_mask_key() {
        let api_key = format!("{}{}", "sk_", "abcdefghij1234");
        assert_eq!(mask_if_secret("key", &api_key), "sk_**********1234");
        assert_eq!(mask_if_secret("remote", "https://x"), "https://x");
    }

    #[test]
    fn test_validate_value_transparency() {
        assert!(validate_value("transparency", "on").is_ok());
        assert!(validate_value("transparency", "off").is_ok());
        assert!(validate_value("transparency", "maybe").is_err());
    }

    #[test]
    fn test_validate_value_api_url() {
        assert!(validate_value("api_url", "https://x").is_ok());
        assert!(validate_value("api_url", "http://x").is_ok());
        assert!(validate_value("api_url", "not a url").is_err());
    }

    #[test]
    fn test_validate_value_remote() {
        assert!(validate_value("remote", "origin").is_ok());
        assert!(validate_value("remote", "oobo").is_ok());
        assert!(validate_value("remote", "git@github.com:org/repo-oobo.git").is_ok());
        assert!(validate_value("remote", "not a url").is_err());
    }

    #[test]
    fn test_validate_value_remote_urls() {
        assert!(validate_value("remote", "git@github.com:org/repo-oobo.git").is_ok());
        assert!(validate_value("remote", "https://github.com/org/repo-oobo.git").is_ok());
        assert!(validate_value("remote", "not a url").is_err());
    }

    #[test]
    fn test_default_key_only_sets_api_key() {
        let mut cfg = Config::default();

        write_default(&mut cfg, "key", "sk_test");
        assert_eq!(cfg.server.api_key, "sk_test");

        unset_default(&mut cfg, "key");
        assert!(cfg.server.api_key.is_empty());
    }

    #[test]
    fn test_default_api_url_sets_server_url() {
        let mut cfg = Config::default();

        write_default(&mut cfg, "api_url", "https://staging.oobo.ai");
        assert_eq!(cfg.server.url, "https://staging.oobo.ai");

        unset_default(&mut cfg, "api_url");
        assert_eq!(cfg.server.url, crate::config::DEFAULT_SERVER_URL);
    }

    #[test]
    fn test_default_remote_sets_anchors_remote() {
        let mut cfg = Config::default();

        write_default(&mut cfg, "remote", "oobo");
        assert_eq!(cfg.anchors.remote, "oobo");

        unset_default(&mut cfg, "remote");
        assert!(cfg.anchors.remote.is_empty());
    }
}
