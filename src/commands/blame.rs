use crate::cli::OutputMode;
use crate::config::Config;
use crate::core::anchor::FileAttribution;
use crate::git::{orphan, proxy};

pub fn run(cfg: &Config, file: &str, commit: Option<&str>, mode: OutputMode) -> Result<(), String> {
    let project_root = proxy::project_root(cfg).ok_or("not inside a git repository")?;

    let commit_hash = match commit {
        Some(rev) => proxy::run_git_capture(cfg, &["rev-parse", rev])
            .map_err(|e| format!("could not resolve '{rev}': {e}"))?,
        None => proxy::run_git_capture(cfg, &["rev-parse", "HEAD"])
            .map_err(|_| "could not resolve HEAD".to_string())?,
    };

    let normalized = normalize_file_path(file, &project_root);

    let anchor = orphan::read_anchor(&project_root, &commit_hash).ok_or_else(|| {
        format!(
            "no anchor metadata for commit {}",
            &commit_hash[..7.min(commit_hash.len())]
        )
    })?;

    let file_change = anchor
        .file_changes
        .iter()
        .find(|fc| fc.path == normalized)
        .ok_or_else(|| {
            format!(
                "file '{normalized}' not found in commit {}",
                &commit_hash[..7.min(commit_hash.len())]
            )
        })?;

    if mode == OutputMode::Json {
        let j = serde_json::to_string_pretty(file_change).map_err(|e| format!("json: {e}"))?;
        println!("{j}");
        return Ok(());
    }

    let file_content =
        proxy::run_git_capture(cfg, &["show", &format!("{commit_hash}:{normalized}")]).ok();

    let lines: Vec<&str> = file_content
        .as_deref()
        .map(|c| c.lines().collect())
        .unwrap_or_default();

    let short_hash = &commit_hash[..7.min(commit_hash.len())];

    if mode == OutputMode::Agent {
        print_agent_output(&normalized, short_hash, file_change, &lines);
        return Ok(());
    }

    print_tui_output(&normalized, short_hash, file_change, &lines);
    Ok(())
}

fn print_tui_output(
    file: &str,
    short_hash: &str,
    fc: &crate::core::anchor::FileChange,
    lines: &[&str],
) {
    let attribution_label = match fc.attribution {
        Some(FileAttribution::Ai) => "AI",
        Some(FileAttribution::Human) => "human",
        Some(FileAttribution::Mixed) => "mixed",
        None => "unknown",
    };
    println!(
        "\x1b[1m{file}\x1b[0m ({short_hash}) — {attribution_label}{}",
        fc.agent
            .as_ref()
            .map(|a| format!(" via {a}"))
            .unwrap_or_default()
    );
    println!();

    if fc.line_attributions.is_empty() {
        println!("  File-level attribution only (no per-line data).");
        if let Some(ref attr) = fc.attribution {
            let (ai_add, ai_del, human_add, human_del) = match attr {
                FileAttribution::Ai => (fc.added, fc.deleted, 0, 0),
                FileAttribution::Human => (0, 0, fc.added, fc.deleted),
                FileAttribution::Mixed => {
                    let ai_a = fc.added / 2;
                    let ai_d = fc.deleted / 2;
                    (ai_a, ai_d, fc.added - ai_a, fc.deleted - ai_d)
                }
            };
            println!("  AI: +{ai_add} -{ai_del}, Human: +{human_add} -{human_del}");
        }
        return;
    }

    if lines.is_empty() {
        println!("  (file content not available)");
        return;
    }

    let line_map = build_line_map(fc);
    let width = format!("{}", lines.len()).len();

    for (i, line_text) in lines.iter().enumerate() {
        let line_num = (i + 1) as u32;
        let (label, color, agent_str) = match line_map.get(&line_num) {
            Some((FileAttribution::Ai, agent)) => {
                let agent_label = agent
                    .as_ref()
                    .map(|a| format!(" {a:<8}"))
                    .unwrap_or_else(|| " ai      ".to_string());
                ("ai   ", "\x1b[36m", agent_label)
            }
            Some((FileAttribution::Human, _)) => ("human", "\x1b[33m", "         ".to_string()),
            Some((FileAttribution::Mixed, agent)) => {
                let agent_label = agent
                    .as_ref()
                    .map(|a| format!(" {a:<8}"))
                    .unwrap_or_else(|| "         ".to_string());
                ("mixed", "\x1b[35m", agent_label)
            }
            None => ("     ", "\x1b[2m", "         ".to_string()),
        };

        println!(
            "{color}{:>width$}\x1b[0m│{color}{label}{agent_str}\x1b[0m│ {line_text}",
            line_num,
        );
    }
}

fn print_agent_output(
    file: &str,
    short_hash: &str,
    fc: &crate::core::anchor::FileChange,
    lines: &[&str],
) {
    let attribution_label = match fc.attribution {
        Some(FileAttribution::Ai) => "ai",
        Some(FileAttribution::Human) => "human",
        Some(FileAttribution::Mixed) => "mixed",
        None => "unknown",
    };
    println!("# {file} ({short_hash}) — {attribution_label}");

    if fc.line_attributions.is_empty() {
        println!("file-level only | no per-line data");
        return;
    }

    if lines.is_empty() {
        println!("(file content not available)");
        return;
    }

    let line_map = build_line_map(fc);

    for (i, line_text) in lines.iter().enumerate() {
        let line_num = (i + 1) as u32;
        let label = match line_map.get(&line_num) {
            Some((FileAttribution::Ai, _)) => "ai   ",
            Some((FileAttribution::Human, _)) => "human",
            Some((FileAttribution::Mixed, _)) => "mixed",
            None => "     ",
        };
        println!("{:>4}|{label}| {line_text}", line_num);
    }
}

/// Build a lookup from line number → (attribution, agent) for fast rendering.
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

/// Normalize a user-supplied file path to the repo-relative form that anchors use.
/// Handles `./src/main.rs`, absolute paths, and trailing slashes.
fn normalize_file_path(file: &str, project_root: &str) -> String {
    let path = std::path::Path::new(file);

    let relative = if path.is_absolute() {
        path.strip_prefix(project_root)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| path.to_path_buf())
    } else {
        let cwd = std::env::current_dir().unwrap_or_default();
        let abs = cwd.join(path);
        abs.strip_prefix(project_root)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| path.to_path_buf())
    };

    relative
        .to_string_lossy()
        .trim_start_matches("./")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::anchor::{FileChange, LineAttribution, LineRange};

    // ── helpers ──────────────────────────────────────────────────────────────

    fn make_file_change(
        attribution: Option<FileAttribution>,
        line_attrs: Vec<LineAttribution>,
    ) -> FileChange {
        FileChange {
            path: "src/lib.rs".to_string(),
            added: 10,
            deleted: 2,
            attribution,
            agent: None,
            line_attributions: line_attrs,
        }
    }

    // ── normalize_file_path ───────────────────────────────────────────────────

    #[test]
    fn test_normalize_strips_dot_slash() {
        let root = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let result = normalize_file_path("./src/main.rs", &root);
        assert_eq!(result, "src/main.rs");
    }

    #[test]
    fn test_normalize_already_clean() {
        let root = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let result = normalize_file_path("src/main.rs", &root);
        assert_eq!(result, "src/main.rs");
    }

    #[test]
    fn test_normalize_absolute_within_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        let abs_file = tmp.path().join("src/main.rs");
        let result = normalize_file_path(&abs_file.to_string_lossy(), &root);
        assert_eq!(result, "src/main.rs");
    }

    #[test]
    fn test_normalize_absolute_outside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        // A path that does not live under root — returned as-is.
        let outside = "/this/is/outside/path.rs";
        let result = normalize_file_path(outside, &root);
        assert_eq!(result, outside);
    }

    // ── build_line_map ────────────────────────────────────────────────────────

    #[test]
    fn test_build_line_map_empty() {
        let fc = make_file_change(None, vec![]);
        let map = build_line_map(&fc);
        assert!(map.is_empty());
    }

    #[test]
    fn test_build_line_map_ai_range() {
        let fc = make_file_change(
            Some(FileAttribution::Ai),
            vec![LineAttribution {
                author: FileAttribution::Ai,
                ranges: vec![LineRange::new(1, 3)],
                agent: None,
            }],
        );
        let map = build_line_map(&fc);
        assert_eq!(map.len(), 3);
        for line in 1u32..=3 {
            assert_eq!(map[&line].0, FileAttribution::Ai);
        }
    }

    #[test]
    fn test_build_line_map_human_range() {
        let fc = make_file_change(
            Some(FileAttribution::Human),
            vec![LineAttribution {
                author: FileAttribution::Human,
                ranges: vec![LineRange::new(2, 4)],
                agent: None,
            }],
        );
        let map = build_line_map(&fc);
        assert_eq!(map.len(), 3);
        for line in 2u32..=4 {
            assert_eq!(map[&line].0, FileAttribution::Human);
        }
    }

    #[test]
    fn test_build_line_map_agent_field_preserved() {
        let fc = make_file_change(
            Some(FileAttribution::Ai),
            vec![LineAttribution {
                author: FileAttribution::Ai,
                ranges: vec![LineRange::new(1, 1)],
                agent: Some("cursor".to_string()),
            }],
        );
        let map = build_line_map(&fc);
        assert_eq!(map[&1].0, FileAttribution::Ai);
        assert_eq!(map[&1].1, Some("cursor".to_string()));
    }

    #[test]
    fn test_build_line_map_non_contiguous_ranges() {
        let fc = make_file_change(
            Some(FileAttribution::Ai),
            vec![LineAttribution {
                author: FileAttribution::Ai,
                ranges: vec![LineRange::new(1, 2), LineRange::new(5, 6)],
                agent: None,
            }],
        );
        let map = build_line_map(&fc);
        assert!(map.contains_key(&1));
        assert!(map.contains_key(&2));
        assert!(!map.contains_key(&3));
        assert!(!map.contains_key(&4));
        assert!(map.contains_key(&5));
        assert!(map.contains_key(&6));
    }

    // ── print_agent_output smoke tests ────────────────────────────────────────

    #[test]
    fn test_print_agent_output_no_line_data() {
        let fc = make_file_change(Some(FileAttribution::Ai), vec![]);
        // Should not panic; prints "file-level only" message.
        print_agent_output("src/lib.rs", "abc1234", &fc, &[]);
    }

    #[test]
    fn test_print_agent_output_with_lines() {
        let fc = make_file_change(
            Some(FileAttribution::Ai),
            vec![LineAttribution {
                author: FileAttribution::Ai,
                ranges: vec![LineRange::new(1, 2)],
                agent: Some("cursor".to_string()),
            }],
        );
        let lines = vec!["fn main() {", "    println!(\"hello\");", "}"];
        // Should not panic; prints per-line blame rows.
        print_agent_output("src/lib.rs", "abc1234", &fc, &lines);
    }

    // ── print_tui_output smoke tests ──────────────────────────────────────────

    #[test]
    fn test_print_tui_output_no_line_data() {
        let fc = make_file_change(Some(FileAttribution::Human), vec![]);
        // Should not panic; prints file-level summary.
        print_tui_output("src/lib.rs", "abc1234", &fc, &[]);
    }

    #[test]
    fn test_print_tui_output_with_lines() {
        let fc = make_file_change(
            Some(FileAttribution::Mixed),
            vec![
                LineAttribution {
                    author: FileAttribution::Ai,
                    ranges: vec![LineRange::new(1, 1)],
                    agent: Some("cursor".to_string()),
                },
                LineAttribution {
                    author: FileAttribution::Human,
                    ranges: vec![LineRange::new(2, 3)],
                    agent: None,
                },
            ],
        );
        let lines = vec!["fn main() {", "    println!(\"hello\");", "}"];
        // Should not panic; renders coloured per-line rows.
        print_tui_output("src/lib.rs", "abc1234", &fc, &lines);
    }
}
