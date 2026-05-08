//! `oobo blame` — strict superset of `git blame`.
//!
//! For every line in the file, we trace it back to the commit that introduced
//! it (via `git blame --porcelain`), load that commit's anchor from the orphan
//! branch, and look up per-line AI/human attribution. This gives historically
//! accurate blame across the full file history — not just the latest commit.
//!
//! Passthrough modes: `--no-ai`, `--porcelain`, `--line-porcelain`,
//! `--incremental` forward to `git blame` byte-for-byte.

use std::collections::HashMap;

use crate::cli::OutputMode;
use crate::config::Config;
use crate::core::anchor::{Anchor, FileAttribution, SessionLink};
use crate::error::{CliError, CmdResult};
use crate::git::{orphan, proxy};

/// Parsed line from `git blame --porcelain`.
struct PorcelainLine {
    orig_sha: String,
    orig_lineno: u32,
    final_lineno: u32,
    filename: String,
    content: String,
}

/// Rich per-line attribution for output.
struct LineBlame {
    final_lineno: u32,
    content: String,
    orig_sha: String,
    attribution: Option<FileAttribution>,
    agent: Option<String>,
    session_ids: Vec<String>,
    total_tokens: u64,
    committed_at: Option<i64>,
    message: Option<String>,
}

/// Entry point.
pub fn run(cfg: &Config, no_ai: bool, args: &[String], mode: OutputMode) -> CmdResult {
    if proxy::project_root(cfg).is_none() {
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
        OutputMode::Json => emit_json(cfg, &file, commit.as_deref(), args),
        OutputMode::Agent => emit_agent(cfg, &file, commit.as_deref(), args),
        OutputMode::Tui => emit_overlay(cfg, &file, commit.as_deref(), args),
    }
}

// ------------------------------------------------------------------
// porcelain parser
// ------------------------------------------------------------------

/// Run `git blame --porcelain` and parse into structured per-line data.
/// Each line gets: originating SHA, original line number, filename in
/// that commit (handles renames), and the source content.
fn run_porcelain(cfg: &Config, file: &str, commit: Option<&str>) -> Result<Vec<PorcelainLine>, CliError> {
    let rev = commit.unwrap_or("HEAD");
    let raw = proxy::run_git_capture(cfg, &["blame", "--porcelain", rev, "--", file])?;

    let mut lines = Vec::new();
    let mut iter = raw.lines().peekable();

    // Track the current block's metadata
    let mut cur_sha = String::new();
    let mut cur_orig: u32 = 0;
    let mut cur_final: u32 = 0;
    let mut cur_filename = String::new();

    while let Some(line) = iter.next() {
        if line.starts_with('\t') {
            // Content line — emit the accumulated entry
            lines.push(PorcelainLine {
                orig_sha: cur_sha.clone(),
                orig_lineno: cur_orig,
                final_lineno: cur_final,
                filename: cur_filename.clone(),
                content: line[1..].to_string(),
            });
            continue;
        }

        // Check if this is a header line (40-char hex SHA followed by numbers)
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 && parts[0].len() == 40 && parts[0].chars().all(|c| c.is_ascii_hexdigit()) {
            cur_sha = parts[0].to_string();
            cur_orig = parts[1].parse().unwrap_or(0);
            cur_final = parts[2].parse().unwrap_or(0);
            continue;
        }

        // Metadata lines
        if let Some(fname) = line.strip_prefix("filename ") {
            cur_filename = fname.to_string();
        }
    }

    Ok(lines)
}

/// Load anchors for a set of commit SHAs, returning a cache map.
/// Only loads each SHA once; returns `None` for commits without anchors.
fn load_anchors(root: &str, shas: &[String]) -> HashMap<String, Option<Anchor>> {
    let mut cache: HashMap<String, Option<Anchor>> = HashMap::new();
    for sha in shas {
        if !cache.contains_key(sha) {
            cache.insert(sha.clone(), orphan::read_anchor(root, sha));
        }
    }
    cache
}

/// Load session links for a commit, summing token counts.
fn session_summary(root: &str, sha: &str) -> (Vec<String>, u64) {
    let links: Vec<SessionLink> = orphan::read_session_links(root, sha);
    let mut ids = Vec::new();
    let mut tokens: u64 = 0;
    for link in &links {
        ids.push(link.session_id.clone());
        tokens += link.input_tokens.unwrap_or(0);
        tokens += link.output_tokens.unwrap_or(0);
    }
    (ids, tokens)
}

/// For a given anchor + filename + line number, find the attribution.
fn lookup_attribution(
    anchor: &Anchor,
    filename: &str,
    orig_lineno: u32,
) -> (Option<FileAttribution>, Option<String>) {
    let fc = match anchor.file_changes.iter().find(|f| f.path == filename) {
        Some(fc) => fc,
        None => return (None, None),
    };
    for la in &fc.line_attributions {
        for range in &la.ranges {
            if orig_lineno >= range.start && orig_lineno <= range.end {
                return (Some(la.author.clone()), la.agent.clone());
            }
        }
    }
    // Line exists in the file change but has no explicit attribution —
    // fall back to the file-level attribution if present.
    (fc.attribution.clone(), fc.agent.clone())
}

/// Build the full blame dataset: for every line in the file, trace back to
/// the originating commit and pull attribution from that commit's anchor.
///
/// When `rich` is true, also loads session links (tokens, session IDs, commit
/// metadata) for the JSON output. Skipped for agent/TUI modes to avoid the
/// extra git subprocess calls per unique commit.
fn build_full_blame(
    cfg: &Config,
    file: &str,
    commit: Option<&str>,
    rich: bool,
) -> Result<(String, Vec<LineBlame>), CliError> {
    let root = proxy::project_root(cfg)
        .ok_or_else(|| CliError::Git("not in a git repo".into()))?;
    let commit_hash = resolve_commit(cfg, commit)?;
    let normalized = normalize_file_path(file, &root);

    let porcelain = run_porcelain(cfg, &normalized, Some(&commit_hash))?;

    let unique_shas: Vec<String> = {
        let mut seen = std::collections::HashSet::new();
        porcelain.iter().filter_map(|p| {
            if seen.insert(p.orig_sha.clone()) { Some(p.orig_sha.clone()) } else { None }
        }).collect()
    };

    let anchor_cache = load_anchors(&root, &unique_shas);

    let mut session_cache: HashMap<String, (Vec<String>, u64)> = HashMap::new();

    let mut result = Vec::with_capacity(porcelain.len());
    for pl in &porcelain {
        let (attribution, agent, session_ids, total_tokens, committed_at, message) =
            match anchor_cache.get(&pl.orig_sha).and_then(|a| a.as_ref()) {
                Some(anchor) => {
                    let (attr, ag) = lookup_attribution(anchor, &pl.filename, pl.orig_lineno);
                    if rich {
                        let (sids, tok) = session_cache
                            .entry(pl.orig_sha.clone())
                            .or_insert_with(|| session_summary(&root, &pl.orig_sha))
                            .clone();
                        (attr, ag, sids, tok, Some(anchor.committed_at), Some(anchor.message.clone()))
                    } else {
                        (attr, ag, Vec::new(), 0, None, None)
                    }
                }
                None => (None, None, Vec::new(), 0, None, None),
            };

        result.push(LineBlame {
            final_lineno: pl.final_lineno,
            content: pl.content.clone(),
            orig_sha: pl.orig_sha.clone(),
            attribution,
            agent,
            session_ids,
            total_tokens,
            committed_at,
            message,
        });
    }

    Ok((normalized, result))
}

// ------------------------------------------------------------------
// passthrough
// ------------------------------------------------------------------

fn passthrough(cfg: &Config, args: &[String]) -> CmdResult {
    let mut argv: Vec<String> = vec!["blame".to_string()];
    argv.extend_from_slice(args);
    let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
    proxy::run_and_intercept(cfg, &borrowed)
}

fn is_machine_output(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "--porcelain" || a == "--line-porcelain" || a == "--incremental")
}

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
// JSON — rich output for plugins and tooling
// ------------------------------------------------------------------

fn emit_json(cfg: &Config, file: &str, commit: Option<&str>, args: &[String]) -> CmdResult {
    let (normalized, blame_lines) = match build_full_blame(cfg, file, commit, true) {
        Ok(v) => v,
        Err(_) => return passthrough(cfg, args),
    };

    let line_entries: Vec<serde_json::Value> = blame_lines
        .iter()
        .map(|lb| {
            let ai_val = match &lb.attribution {
                Some(FileAttribution::Ai) => serde_json::json!(lb.agent.as_deref().unwrap_or("ai")),
                Some(FileAttribution::Mixed) => serde_json::json!(lb.agent.as_deref().unwrap_or("mixed")),
                Some(FileAttribution::Human) => serde_json::json!("human"),
                None => serde_json::Value::Null,
            };

            let mut entry = serde_json::json!({
                "line": lb.final_lineno,
                "content": lb.content,
                "origin_sha": short(&lb.orig_sha),
                "ai": ai_val,
            });

            if let Some(agent) = &lb.agent {
                entry["agent"] = serde_json::json!(agent);
            }
            if !lb.session_ids.is_empty() {
                entry["session_ids"] = serde_json::json!(lb.session_ids);
            }
            if lb.total_tokens > 0 {
                entry["tokens"] = serde_json::json!(lb.total_tokens);
            }
            if let Some(ts) = lb.committed_at {
                entry["committed_at"] = serde_json::json!(ts);
            }
            if let Some(msg) = &lb.message {
                entry["message"] = serde_json::json!(msg);
            }

            entry
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
// Agent — compact text columns
// ------------------------------------------------------------------

fn emit_agent(cfg: &Config, file: &str, commit: Option<&str>, args: &[String]) -> CmdResult {
    let (_, blame_lines) = match build_full_blame(cfg, file, commit, false) {
        Ok(v) => v,
        Err(_) => return passthrough(cfg, args),
    };

    for lb in &blame_lines {
        let sha7 = short(&lb.orig_sha);
        let attr = match &lb.attribution {
            Some(FileAttribution::Ai) => lb.agent.clone().unwrap_or_else(|| "ai".into()),
            Some(FileAttribution::Mixed) => lb.agent.clone().unwrap_or_else(|| "mixed".into()),
            Some(FileAttribution::Human) => "human".into(),
            None => "-".into(),
        };
        println!("{sha7} {attr:<7} {:>4}  {}", lb.final_lineno, lb.content);
    }
    Ok(0)
}

// ------------------------------------------------------------------
// TUI (overlay) — colored git-blame with AI column
// ------------------------------------------------------------------

fn emit_overlay(cfg: &Config, file: &str, commit: Option<&str>, args: &[String]) -> CmdResult {
    let (_, blame_lines) = match build_full_blame(cfg, file, commit, false) {
        Ok(v) => v,
        Err(_) => return passthrough(cfg, args),
    };

    // Run normal git blame for the formatted output
    let mut blame_argv: Vec<String> = vec!["blame".to_string()];
    blame_argv.extend_from_slice(args);
    let borrowed: Vec<&str> = blame_argv.iter().map(String::as_str).collect();
    let raw = match proxy::run_git_capture(cfg, &borrowed) {
        Ok(s) => s,
        Err(_) => return passthrough(cfg, args),
    };

    // Index blame_lines by final_lineno for O(1) lookup
    let attr_map: HashMap<u32, &LineBlame> = blame_lines
        .iter()
        .map(|lb| (lb.final_lineno, lb))
        .collect();

    for raw_line in raw.lines() {
        let n = parse_blame_line_number(raw_line);
        let attr_str = n
            .and_then(|n| attr_map.get(&n))
            .map(|lb| format_attr_colored(lb))
            .unwrap_or_else(|| "\x1b[90m-\x1b[0m       ".to_string());
        println!("{attr_str} {raw_line}");
    }
    Ok(0)
}

fn format_attr_colored(lb: &LineBlame) -> String {
    match &lb.attribution {
        Some(FileAttribution::Ai) => {
            let label = lb.agent.as_deref().unwrap_or("ai");
            format!("\x1b[35m{label:<8}\x1b[0m") // magenta for AI
        }
        Some(FileAttribution::Mixed) => {
            let label = lb.agent.as_deref().unwrap_or("mixed");
            format!("\x1b[33m{label:<8}\x1b[0m") // yellow for mixed
        }
        Some(FileAttribution::Human) => {
            format!("\x1b[32m{:<8}\x1b[0m", "human") // green for human
        }
        None => {
            format!("\x1b[90m{:<8}\x1b[0m", "-") // dim for unknown
        }
    }
}

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

fn normalize_file_path(file: &str, project_root: &str) -> String {
    let path = std::path::Path::new(file);
    let relative = if path.is_absolute() {
        path.strip_prefix(project_root)
            .map_or_else(|_| path.to_path_buf(), std::path::Path::to_path_buf)
    } else {
        let cwd = std::env::current_dir().unwrap_or_default();
        let abs = cwd.join(path);
        abs.strip_prefix(project_root)
            .map_or_else(|_| path.to_path_buf(), std::path::Path::to_path_buf)
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

    #[test]
    fn test_porcelain_parser() {
        // Minimal porcelain output: one commit block, one line
        let porcelain_output = "\
a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 1 1 1\n\
author Teddy\n\
author-mail <teddy@example.com>\n\
author-time 1700000000\n\
author-tz +0000\n\
committer Teddy\n\
committer-mail <teddy@example.com>\n\
committer-time 1700000000\n\
committer-tz +0000\n\
summary initial commit\n\
filename src/main.rs\n\
\tfn main() {}";

        let mut lines = Vec::new();
        let mut iter = porcelain_output.lines().peekable();
        let mut cur_sha = String::new();
        let mut cur_orig: u32 = 0;
        let mut cur_final: u32 = 0;
        let mut cur_filename = String::new();

        while let Some(line) = iter.next() {
            if line.starts_with('\t') {
                lines.push(PorcelainLine {
                    orig_sha: cur_sha.clone(),
                    orig_lineno: cur_orig,
                    final_lineno: cur_final,
                    filename: cur_filename.clone(),
                    content: line[1..].to_string(),
                });
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 && parts[0].len() == 40 && parts[0].chars().all(|c| c.is_ascii_hexdigit()) {
                cur_sha = parts[0].to_string();
                cur_orig = parts[1].parse().unwrap_or(0);
                cur_final = parts[2].parse().unwrap_or(0);
                continue;
            }
            if let Some(fname) = line.strip_prefix("filename ") {
                cur_filename = fname.to_string();
            }
        }

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].orig_sha, "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2");
        assert_eq!(lines[0].orig_lineno, 1);
        assert_eq!(lines[0].final_lineno, 1);
        assert_eq!(lines[0].filename, "src/main.rs");
        assert_eq!(lines[0].content, "fn main() {}");
    }

    #[test]
    fn test_format_attr_colored() {
        let lb = LineBlame {
            final_lineno: 1,
            content: String::new(),
            orig_sha: "abc".into(),
            attribution: Some(FileAttribution::Ai),
            agent: Some("cursor".into()),
            session_ids: Vec::new(),
            total_tokens: 0,
            committed_at: None,
            message: None,
        };
        let s = format_attr_colored(&lb);
        assert!(s.contains("cursor"));

        let lb_human = LineBlame {
            attribution: Some(FileAttribution::Human),
            agent: None,
            ..lb
        };
        let s = format_attr_colored(&lb_human);
        assert!(s.contains("human"));
    }
}
