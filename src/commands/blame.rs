//! `oobo blame` — strict superset of `git blame`.
//!
//! We forward every arg to `git blame` and, by default, prepend an AI-column
//! attribution (the tool that wrote each line, or `human`). In a handful of
//! cases we must NOT modify the output: `--no-ai`, `--porcelain`,
//! `--line-porcelain`, and `--incremental` are passed through byte-for-byte.

use crate::cli::OutputMode;
use crate::config::Config;
use crate::core::anchor::FileAttribution;
use crate::error::{CliError, CmdResult};
use crate::git::{orphan, proxy};

/// Entry point.
///
/// - `no_ai = true`  → pure git-blame passthrough.
/// - porcelain-family flag in `args` → pure passthrough.
/// - `mode == Json` → structured JSON output.
/// - `mode == Agent` → flat columns.
/// - `mode == Tui` → colored git-blame + AI column.
pub fn run(cfg: &Config, no_ai: bool, args: &[String], mode: OutputMode) -> CmdResult {
    if crate::git::proxy::project_root(cfg).is_none() {
        eprintln!("oobo: not inside a git repository.");
        return Ok(1);
    }

    if no_ai || is_machine_output(args) {
        return passthrough(cfg, args);
    }

    let (file, commit) = detect_file_and_commit(args);
    if file.is_empty() {
        return passthrough(cfg, args);
    }

    match mode {
        OutputMode::Json => emit_json(cfg, &file, commit.as_deref()),
        OutputMode::Agent => emit_agent(cfg, &file, commit.as_deref(), args),
        OutputMode::Tui => emit_overlay(cfg, &file, commit.as_deref(), args),
    }
}

// ------------------------------------------------------------------
// passthrough
// ------------------------------------------------------------------

/// Exec `git blame <args>` and forward stdout/stderr and the exit code.
fn passthrough(cfg: &Config, args: &[String]) -> CmdResult {
    let mut argv: Vec<String> = vec!["blame".to_string()];
    for a in args {
        argv.push(a.clone());
    }
    let borrowed: Vec<&str> = argv.iter().map(std::string::String::as_str).collect();
    proxy::run_and_intercept(cfg, &borrowed)
}

fn is_machine_output(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "--porcelain" || a == "--line-porcelain" || a == "--incremental")
}

/// Scan passed args for the first positional that looks like a file path.
/// Returns `(file, commit)` where `commit` is optional.
fn detect_file_and_commit(args: &[String]) -> (String, Option<String>) {
    let mut positionals: Vec<String> = Vec::new();
    let mut iter = args.iter().peekable();
    while let Some(a) = iter.next() {
        if a == "--" {
            for rest in iter.by_ref() {
                positionals.push(rest.clone());
            }
            break;
        }
        if a.starts_with('-') {
            if matches!(
                a.as_str(),
                "-L" | "--abbrev" | "--date" | "--since" | "--until"
            ) {
                iter.next();
            }
            continue;
        }
        positionals.push(a.clone());
    }
    match positionals.len() {
        0 => (String::new(), None),
        1 => (positionals.remove(0), None),
        _ => {
            let file = positionals.pop().unwrap();
            let commit = positionals.pop();
            (file, commit)
        }
    }
}

// ------------------------------------------------------------------
// JSON
// ------------------------------------------------------------------

fn emit_json(cfg: &Config, file: &str, commit: Option<&str>) -> CmdResult {
    let root = match proxy::project_root(cfg) {
        Some(r) => r,
        None => return passthrough(cfg, &[file.to_string()]),
    };
    let commit_hash = resolve_commit(cfg, commit)?;
    let normalized = normalize_file_path(file, &root);

    let anchor = orphan::read_anchor(&root, &commit_hash);
    let file_content =
        proxy::run_git_capture(cfg, &["show", &format!("{commit_hash}:{normalized}")]).ok();
    let lines: Vec<&str> = file_content
        .as_deref()
        .map(|c| c.lines().collect())
        .unwrap_or_default();

    let line_map = anchor
        .as_ref()
        .and_then(|a| a.file_changes.iter().find(|f| f.path == normalized))
        .map(build_line_map)
        .unwrap_or_default();

    let line_entries: Vec<serde_json::Value> = lines
        .iter()
        .enumerate()
        .map(|(i, content)| {
            let n = (i + 1) as u32;
            let entry = line_map.get(&n);
            let (ai_val, agent) = match entry {
                Some((FileAttribution::Ai, a)) => (
                    serde_json::Value::String(a.clone().unwrap_or_else(|| "ai".into())),
                    a.clone(),
                ),
                Some((FileAttribution::Mixed, a)) => (
                    serde_json::Value::String(a.clone().unwrap_or_else(|| "mixed".into())),
                    a.clone(),
                ),
                Some((FileAttribution::Human, _)) | None => (serde_json::Value::Null, None),
            };
            let _ = agent;
            serde_json::json!({
                "line": n,
                "sha": short(&commit_hash),
                "ai": ai_val,
                "content": content,
            })
        })
        .collect();

    let json = serde_json::json!({
        "file": normalized,
        "commit": commit.unwrap_or("HEAD"),
        "lines": line_entries,
    });
    crate::utils::print_json(&json);
    Ok(0)
}

// ------------------------------------------------------------------
// Agent
// ------------------------------------------------------------------

fn emit_agent(
    cfg: &Config,
    file: &str,
    commit: Option<&str>,
    args: &[String],
) -> CmdResult {
    let root = match proxy::project_root(cfg) {
        Some(r) => r,
        None => return passthrough(cfg, args),
    };
    let commit_hash = resolve_commit(cfg, commit)?;
    let normalized = normalize_file_path(file, &root);
    let anchor = orphan::read_anchor(&root, &commit_hash);

    let file_content =
        proxy::run_git_capture(cfg, &["show", &format!("{commit_hash}:{normalized}")]).ok();
    let lines: Vec<&str> = file_content
        .as_deref()
        .map(|c| c.lines().collect())
        .unwrap_or_default();

    let line_map = anchor
        .as_ref()
        .and_then(|a| a.file_changes.iter().find(|f| f.path == normalized))
        .map(build_line_map)
        .unwrap_or_default();

    let sha7 = short(&commit_hash);
    for (i, line_text) in lines.iter().enumerate() {
        let n = (i + 1) as u32;
        let attr = match line_map.get(&n) {
            Some((FileAttribution::Ai, agent)) => agent.clone().unwrap_or_else(|| "ai".into()),
            Some((FileAttribution::Mixed, agent)) => {
                agent.clone().unwrap_or_else(|| "mixed".into())
            }
            Some((FileAttribution::Human, _)) => "human".into(),
            None => "-".into(),
        };
        println!("{sha7} {attr:<7} {n:>4}  {line_text}");
    }
    Ok(0)
}

// ------------------------------------------------------------------
// TUI (overlay)
// ------------------------------------------------------------------

fn emit_overlay(
    cfg: &Config,
    file: &str,
    commit: Option<&str>,
    args: &[String],
) -> CmdResult {
    let root = match proxy::project_root(cfg) {
        Some(r) => r,
        None => return passthrough(cfg, args),
    };
    let commit_hash = resolve_commit(cfg, commit)?;
    let normalized = normalize_file_path(file, &root);
    let anchor = orphan::read_anchor(&root, &commit_hash);

    // Capture git blame output verbatim; we'll add a leading column.
    let mut blame_argv: Vec<String> = vec!["blame".to_string()];
    for a in args {
        blame_argv.push(a.clone());
    }
    let borrowed: Vec<&str> = blame_argv.iter().map(std::string::String::as_str).collect();
    let raw = match proxy::run_git_capture(cfg, &borrowed) {
        Ok(s) => s,
        Err(_) => return passthrough(cfg, args),
    };

    let line_map = anchor
        .as_ref()
        .and_then(|a| a.file_changes.iter().find(|f| f.path == normalized))
        .map(build_line_map)
        .unwrap_or_default();

    for raw_line in raw.lines() {
        let n = parse_blame_line_number(raw_line);
        let attr = n
            .and_then(|n| line_map.get(&n)).map_or_else(|| "-".to_string(), |(a, agent)| format_attr(a, agent.as_ref()));
        println!("\x1b[36m{attr:<8}\x1b[0m {raw_line}");
    }
    Ok(0)
}

fn format_attr(a: &FileAttribution, agent: Option<&String>) -> String {
    match a {
        FileAttribution::Ai => agent.cloned().unwrap_or_else(|| "ai".into()),
        FileAttribution::Mixed => agent.cloned().unwrap_or_else(|| "mixed".into()),
        FileAttribution::Human => "human".into(),
    }
}

/// Best-effort extractor of the source-line number from a `git blame` line.
/// Default `git blame` output looks like:
///   a1b2c3d (Author 2024-01-01 14:03:12 +0000   12) source
/// We match the token directly before the closing ')'.
fn parse_blame_line_number(line: &str) -> Option<u32> {
    let close = line.find(')')?;
    let head = &line[..close];
    let start = head.rfind(' ').map_or(0, |i| i + 1);
    head[start..].parse::<u32>().ok()
}

// ------------------------------------------------------------------
// shared helpers
// ------------------------------------------------------------------

fn resolve_commit(cfg: &Config, commit: Option<&str>) -> Result<String, CliError> {
    let rev = commit.unwrap_or("HEAD");
    proxy::run_git_capture(cfg, &["rev-parse", rev])
        .map(|s| s.trim().to_string())
        .map_err(|_| CliError::Git(format!("could not resolve '{rev}'")))
}

fn build_line_map(
    fc: &crate::core::anchor::FileChange,
) -> std::collections::HashMap<u32, (FileAttribution, Option<String>)> {
    let mut map = std::collections::HashMap::new();
    for la in &fc.line_attributions {
        for range in &la.ranges {
            for line in range.start..=range.end {
                map.insert(line, (la.author.clone(), la.agent.clone()));
            }
        }
    }
    map
}

fn normalize_file_path(file: &str, project_root: &str) -> String {
    let path = std::path::Path::new(file);
    let relative = if path.is_absolute() {
        path.strip_prefix(project_root).map_or_else(|_| path.to_path_buf(), std::path::Path::to_path_buf)
    } else {
        let cwd = std::env::current_dir().unwrap_or_default();
        let abs = cwd.join(path);
        abs.strip_prefix(project_root).map_or_else(|_| path.to_path_buf(), std::path::Path::to_path_buf)
    };
    relative
        .to_string_lossy()
        .trim_start_matches("./")
        .to_string()
}

fn short(sha: &str) -> String {
    if sha.len() >= 7 {
        sha[..7].to_string()
    } else {
        sha.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_file_only() {
        let a = vec!["src/main.rs".to_string()];
        let (f, c) = detect_file_and_commit(&a);
        assert_eq!(f, "src/main.rs");
        assert_eq!(c, None);
    }

    #[test]
    fn test_detect_file_and_commit() {
        let a = vec!["a1b2c3d".to_string(), "src/main.rs".to_string()];
        let (f, c) = detect_file_and_commit(&a);
        assert_eq!(f, "src/main.rs");
        assert_eq!(c.as_deref(), Some("a1b2c3d"));
    }

    #[test]
    fn test_detect_skips_flags() {
        let a = vec!["-w".to_string(), "src/main.rs".to_string()];
        let (f, c) = detect_file_and_commit(&a);
        assert_eq!(f, "src/main.rs");
        assert_eq!(c, None);
    }

    #[test]
    fn test_detect_flag_with_value() {
        let a = vec![
            "-L".to_string(),
            "10,20".to_string(),
            "src/main.rs".to_string(),
        ];
        let (f, _) = detect_file_and_commit(&a);
        assert_eq!(f, "src/main.rs");
    }

    #[test]
    fn test_machine_flags_bypass() {
        assert!(is_machine_output(&["--porcelain".to_string()]));
        assert!(is_machine_output(&["--line-porcelain".to_string()]));
        assert!(is_machine_output(&["--incremental".to_string()]));
        assert!(!is_machine_output(&["-w".to_string()]));
    }

    #[test]
    fn test_parse_blame_line_number() {
        let line = "a1b2c3d (Teddy 2024-01-01 14:03:12 +0000  12) fn main() {";
        assert_eq!(parse_blame_line_number(line), Some(12));
    }
}
