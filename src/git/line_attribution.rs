use crate::config::Config;
use crate::core::anchor::{FileAttribution, LineAttribution, LineRange};

use super::proxy;

/// Build line attributions for pure AI files (agent_blob == committed_blob).
/// All added lines (baseline -> committed) are AI-authored.
pub(in crate::git) fn compute_pure_ai_line_attrs(
    cfg: &Config,
    baseline_blob: &str,
    committed_blob: &str,
    agent_name: Option<&str>,
) -> Vec<LineAttribution> {
    let ai_ranges = if baseline_blob.is_empty() {
        // New file: every line is AI.
        blob_total_range(cfg, committed_blob)
            .map(|r| vec![r])
            .unwrap_or_default()
    } else {
        diff_blobs_added_ranges(cfg, baseline_blob, committed_blob).unwrap_or_default()
    };

    if ai_ranges.is_empty() {
        return Vec::new();
    }

    vec![LineAttribution {
        author: FileAttribution::Ai,
        ranges: ai_ranges,
        agent: agent_name.map(std::string::ToString::to_string),
    }]
}

/// Build line attributions for mixed files (both AI and human contributed).
/// Uses committed-file coordinates for all ranges:
/// - all_added = diff(baseline -> committed): every added line in the commit
/// - human_modified = diff(agent -> committed): lines human changed after the agent
/// - ai_ranges = all_added minus human_modified
pub(in crate::git) fn compute_mixed_line_attrs(
    cfg: &Config,
    baseline_blob: &str,
    agent_blob: &str,
    committed_blob: &str,
    agent_name: Option<&str>,
) -> Vec<LineAttribution> {
    if committed_blob.is_empty() {
        return Vec::new();
    }

    let all_added = if baseline_blob.is_empty() {
        blob_total_range(cfg, committed_blob)
            .map(|r| vec![r])
            .unwrap_or_default()
    } else {
        diff_blobs_added_ranges(cfg, baseline_blob, committed_blob).unwrap_or_default()
    };

    if all_added.is_empty() {
        return Vec::new();
    }

    let human_ranges = diff_blobs_added_ranges(cfg, agent_blob, committed_blob).unwrap_or_default();

    if human_ranges.is_empty() {
        return vec![LineAttribution {
            author: FileAttribution::Ai,
            ranges: all_added,
            agent: agent_name.map(std::string::ToString::to_string),
        }];
    }

    let ai_ranges = subtract_ranges(&all_added, &human_ranges);

    let mut attrs = Vec::new();
    if !ai_ranges.is_empty() {
        attrs.push(LineAttribution {
            author: FileAttribution::Ai,
            ranges: ai_ranges,
            agent: agent_name.map(std::string::ToString::to_string),
        });
    }
    if !human_ranges.is_empty() {
        attrs.push(LineAttribution {
            author: FileAttribution::Human,
            ranges: human_ranges,
            agent: None,
        });
    }
    attrs
}

/// Compute line attributions using per-edit blob pairs from preToolUse/postToolUse.
///
/// Each `FileEditPair` gives us the exact file state before and after a single
/// AI tool call. We diff each pair to find what the AI changed, then project
/// those ranges onto the final committed file to handle subsequent human edits.
///
/// This is more precise than `compute_mixed_line_attrs` because it distinguishes
/// AI edits at tool-call granularity rather than treating the entire turn as one diff.
pub(in crate::git) fn compute_chain_line_attrs(
    cfg: &Config,
    chain: &[crate::hooks::state::FileEditPair],
    baseline_blob: &str,
    committed_blob: &str,
    agent_name: Option<&str>,
) -> Vec<LineAttribution> {
    if committed_blob.is_empty() || chain.is_empty() {
        return Vec::new();
    }

    // Collect all AI-added ranges by diffing each edit pair.
    // These ranges are in the coordinate space of each pair's post_blob.
    // We'll use the final agent blob (last pair's post_blob) to project onto committed.
    let mut all_ai_ranges: Vec<LineRange> = Vec::new();

    for pair in chain {
        let null_blob = "0000000000000000000000000000000000000000";
        let ranges = if pair.pre_blob == null_blob {
            // New file created by the AI — all lines are AI.
            blob_total_range(cfg, &pair.post_blob)
                .map(|r| vec![r])
                .unwrap_or_default()
        } else {
            diff_blobs_added_ranges(cfg, &pair.pre_blob, &pair.post_blob).unwrap_or_default()
        };
        all_ai_ranges.extend(ranges);
    }

    if all_ai_ranges.is_empty() {
        return Vec::new();
    }

    // The AI ranges above are in intermediate blob coordinates. For the final
    // attribution, diff the last agent state against committed to find human
    // post-edits, then subtract those from the total added lines.
    let last_agent_blob = &chain.last().unwrap().post_blob;
    let all_added = if baseline_blob.is_empty() {
        blob_total_range(cfg, committed_blob)
            .map(|r| vec![r])
            .unwrap_or_default()
    } else {
        diff_blobs_added_ranges(cfg, baseline_blob, committed_blob).unwrap_or_default()
    };

    if all_added.is_empty() {
        return Vec::new();
    }

    let human_ranges =
        diff_blobs_added_ranges(cfg, last_agent_blob, committed_blob).unwrap_or_default();
    let ai_ranges = subtract_ranges(&all_added, &human_ranges);

    let mut attrs = Vec::new();
    if !ai_ranges.is_empty() {
        attrs.push(LineAttribution {
            author: FileAttribution::Ai,
            ranges: ai_ranges,
            agent: agent_name.map(std::string::ToString::to_string),
        });
    }
    if !human_ranges.is_empty() {
        attrs.push(LineAttribution {
            author: FileAttribution::Human,
            ranges: human_ranges,
            agent: None,
        });
    }
    attrs
}

/// Subtract `remove` ranges from `source` ranges.
/// All ranges are sorted, non-overlapping, and 1-indexed inclusive.
/// Returns the portions of `source` that don't overlap with `remove`.
fn subtract_ranges(source: &[LineRange], remove: &[LineRange]) -> Vec<LineRange> {
    if remove.is_empty() {
        return source.to_vec();
    }

    let mut result = Vec::new();
    let mut ri = 0;

    for s in source {
        let mut cur_start = s.start;
        let cur_end = s.end;

        while ri < remove.len() && remove[ri].end < cur_start {
            ri += 1;
        }

        let mut rj = ri;
        while rj < remove.len() && remove[rj].start <= cur_end {
            let r = &remove[rj];
            if r.start > cur_start {
                result.push(LineRange::new(cur_start, r.start - 1));
            }
            cur_start = r.end + 1;
            rj += 1;
        }

        if cur_start <= cur_end {
            result.push(LineRange::new(cur_start, cur_end));
        }
    }

    result
}

/// Count lines in a git blob object.
fn count_blob_lines(cfg: &Config, blob: &str) -> Option<u32> {
    if blob.is_empty() {
        return None;
    }
    let content = proxy::run_git_capture(cfg, &["cat-file", "-p", blob]).ok()?;
    Some(content.lines().count() as u32)
}

/// Diff two blob objects and return the line ranges that were added in blob_b.
/// Uses `git diff -U0` to get minimal hunks with precise `@@` headers.
fn diff_blobs_added_ranges(cfg: &Config, blob_a: &str, blob_b: &str) -> Option<Vec<LineRange>> {
    if blob_a.is_empty() || blob_b.is_empty() {
        return None;
    }

    let output = proxy::run_git_capture(cfg, &["diff", "-U0", blob_a, blob_b]).ok()?;
    Some(parse_diff_added_ranges(&output))
}

/// Parse unified diff output (with -U0) and extract added line ranges.
/// Hunk headers look like: `@@ -old_start[,old_count] +new_start[,new_count] @@`
fn parse_diff_added_ranges(diff_output: &str) -> Vec<LineRange> {
    let mut ranges = Vec::new();

    for line in diff_output.lines() {
        if !line.starts_with("@@") {
            continue;
        }
        if let Some(plus_part) = extract_hunk_plus(line) {
            let (start, count) = parse_hunk_range(plus_part);
            if count > 0 {
                ranges.push(LineRange::new(start, start + count - 1));
            }
        }
    }

    ranges
}

/// Extract the `+start[,count]` portion from a `@@ ... @@` hunk header.
fn extract_hunk_plus(hunk_line: &str) -> Option<&str> {
    let after_at = hunk_line.strip_prefix("@@")?;
    let plus_idx = after_at.find('+')?;
    let rest = &after_at[plus_idx + 1..];
    let end = rest.find([' ', '@']).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Parse a hunk range like "42,5" or "42" into (start, count).
/// A missing count means 1 (single-line hunk). A count of 0 means pure deletion.
fn parse_hunk_range(s: &str) -> (u32, u32) {
    if let Some((start_s, count_s)) = s.split_once(',') {
        let start = start_s.parse::<u32>().unwrap_or(1);
        let count = count_s.parse::<u32>().unwrap_or(1);
        (start, count)
    } else {
        let start = s.parse::<u32>().unwrap_or(1);
        (start, 1)
    }
}

/// Get all line numbers in a blob (1..=line_count) as a single LineRange.
fn blob_total_range(cfg: &Config, blob: &str) -> Option<LineRange> {
    let count = count_blob_lines(cfg, blob)?;
    if count == 0 {
        return None;
    }
    Some(LineRange::new(1, count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_diff_added_ranges_single_hunk() {
        let diff = "\
diff --git a/blob1 b/blob2
--- a/blob1
+++ b/blob2
@@ -1,3 +1,5 @@
 existing line
+new line 1
+new line 2
 another existing
";
        let ranges = parse_diff_added_ranges(diff);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 1);
        assert_eq!(ranges[0].end, 5);
    }

    #[test]
    fn parse_diff_added_ranges_multiple_hunks() {
        let diff = "\
@@ -0,0 +1,3 @@
+line1
+line2
+line3
@@ -10,0 +14,2 @@
+inserted1
+inserted2
";
        let ranges = parse_diff_added_ranges(diff);
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0].start, 1);
        assert_eq!(ranges[0].end, 3);
        assert_eq!(ranges[1].start, 14);
        assert_eq!(ranges[1].end, 15);
    }

    #[test]
    fn parse_diff_added_ranges_single_line() {
        let diff = "@@ -5,0 +6 @@\n+one new line\n";
        let ranges = parse_diff_added_ranges(diff);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 6);
        assert_eq!(ranges[0].end, 6);
    }

    #[test]
    fn parse_diff_added_ranges_deletion_only() {
        let diff = "@@ -1,3 +1,0 @@\n-deleted1\n-deleted2\n-deleted3\n";
        let ranges = parse_diff_added_ranges(diff);
        assert!(ranges.is_empty());
    }

    #[test]
    fn parse_diff_added_ranges_empty() {
        assert!(parse_diff_added_ranges("").is_empty());
    }

    #[test]
    fn extract_hunk_plus_standard() {
        assert_eq!(extract_hunk_plus("@@ -1,3 +4,5 @@"), Some("4,5"));
    }

    #[test]
    fn extract_hunk_plus_no_count() {
        assert_eq!(extract_hunk_plus("@@ -1 +4 @@"), Some("4"));
    }

    #[test]
    fn extract_hunk_plus_with_context() {
        assert_eq!(extract_hunk_plus("@@ -1,3 +4,5 @@ fn main()"), Some("4,5"));
    }

    #[test]
    fn parse_hunk_range_with_count() {
        assert_eq!(parse_hunk_range("42,5"), (42, 5));
    }

    #[test]
    fn parse_hunk_range_no_count() {
        assert_eq!(parse_hunk_range("42"), (42, 1));
    }

    #[test]
    fn parse_hunk_range_zero_count() {
        assert_eq!(parse_hunk_range("10,0"), (10, 0));
    }

    #[test]
    fn subtract_ranges_no_overlap() {
        let source = vec![
            LineRange { start: 1, end: 5 },
            LineRange { start: 10, end: 15 },
        ];
        let remove = vec![LineRange { start: 6, end: 9 }];
        let result = subtract_ranges(&source, &remove);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], LineRange { start: 1, end: 5 });
        assert_eq!(result[1], LineRange { start: 10, end: 15 });
    }

    #[test]
    fn subtract_ranges_full_overlap() {
        let source = vec![LineRange { start: 5, end: 10 }];
        let remove = vec![LineRange { start: 3, end: 12 }];
        let result = subtract_ranges(&source, &remove);
        assert!(result.is_empty());
    }

    #[test]
    fn subtract_ranges_partial_overlap() {
        let source = vec![LineRange { start: 1, end: 10 }];
        let remove = vec![LineRange { start: 4, end: 6 }];
        let result = subtract_ranges(&source, &remove);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], LineRange { start: 1, end: 3 });
        assert_eq!(result[1], LineRange { start: 7, end: 10 });
    }

    #[test]
    fn subtract_ranges_empty_remove() {
        let source = vec![LineRange { start: 1, end: 5 }];
        let result = subtract_ranges(&source, &[]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], LineRange { start: 1, end: 5 });
    }

    #[test]
    fn subtract_ranges_multiple_removals() {
        let source = vec![LineRange { start: 1, end: 20 }];
        let remove = vec![
            LineRange { start: 3, end: 5 },
            LineRange { start: 10, end: 12 },
        ];
        let result = subtract_ranges(&source, &remove);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], LineRange { start: 1, end: 2 });
        assert_eq!(result[1], LineRange { start: 6, end: 9 });
        assert_eq!(result[2], LineRange { start: 13, end: 20 });
    }

    #[test]
    fn subtract_ranges_adjacent_no_overlap() {
        let source = vec![LineRange { start: 6, end: 10 }];
        let remove = vec![LineRange { start: 1, end: 5 }];
        let result = subtract_ranges(&source, &remove);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], LineRange { start: 6, end: 10 });
    }

    #[test]
    fn subtract_ranges_shared_boundary() {
        let source = vec![LineRange { start: 5, end: 15 }];
        let remove = vec![LineRange { start: 5, end: 10 }];
        let result = subtract_ranges(&source, &remove);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], LineRange { start: 11, end: 15 });
    }

    #[test]
    fn subtract_ranges_single_point_exact() {
        let source = vec![LineRange { start: 5, end: 5 }];
        let remove = vec![LineRange { start: 5, end: 5 }];
        let result = subtract_ranges(&source, &remove);
        assert!(result.is_empty());
    }

    #[test]
    fn subtract_ranges_remove_tail() {
        let source = vec![LineRange { start: 1, end: 10 }];
        let remove = vec![LineRange { start: 8, end: 10 }];
        let result = subtract_ranges(&source, &remove);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], LineRange { start: 1, end: 7 });
    }

    fn make_test_cfg(repo_path: &std::path::Path) -> (Config, std::path::PathBuf) {
        let mut cfg = Config::default();
        cfg.git.real_git_path = "git".to_string();
        let prev_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(repo_path).unwrap();
        (cfg, prev_cwd)
    }

    static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn init_test_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::process::Command::new("git")
            .args(["init", "--initial-branch=main"])
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        // Initial commit so blob objects can be referenced.
        std::fs::write(root.join(".gitkeep"), "").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        dir
    }

    fn write_blob(repo_path: &std::path::Path, content: &str) -> String {
        let file_path = repo_path.join("_tmp_blob_input");
        std::fs::write(&file_path, content).unwrap();
        let output = std::process::Command::new("git")
            .args(["hash-object", "-w"])
            .arg(&file_path)
            .current_dir(repo_path)
            .stdout(std::process::Stdio::piped())
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn test_compute_chain_line_attrs_empty_chain() {
        let _guard = CWD_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = init_test_repo();
        let (cfg, prev_cwd) = make_test_cfg(dir.path());

        let blob = write_blob(dir.path(), "line1\nline2\n");
        let result = compute_chain_line_attrs(&cfg, &[], "", &blob, Some("cursor"));

        std::env::set_current_dir(&prev_cwd).unwrap();
        assert!(result.is_empty(), "empty chain should return empty vec");
    }

    #[test]
    fn test_compute_chain_line_attrs_new_file() {
        use crate::core::anchor::FileAttribution;
        use crate::hooks::state::FileEditPair;

        let _guard = CWD_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = init_test_repo();
        let (cfg, prev_cwd) = make_test_cfg(dir.path());

        let null_blob = "0000000000000000000000000000000000000000";
        let post_blob = write_blob(dir.path(), "line1\nline2\nline3\n");

        let chain = vec![FileEditPair {
            pre_blob: null_blob.to_string(),
            post_blob: post_blob.clone(),
            tool_name: Some("Write".to_string()),
            timestamp: 1000,
        }];

        let result = compute_chain_line_attrs(&cfg, &chain, "", &post_blob, Some("cursor"));
        assert!(!result.is_empty(), "new file chain should attribute lines");

        let ai_attr = result
            .iter()
            .find(|a| a.author == FileAttribution::Ai)
            .expect("should have AI attribution");
        assert_eq!(ai_attr.ranges.len(), 1);
        assert_eq!(ai_attr.ranges[0].start, 1);
        assert_eq!(ai_attr.ranges[0].end, 3);
        assert_eq!(ai_attr.agent.as_deref(), Some("cursor"));

        std::env::set_current_dir(&prev_cwd).unwrap();
    }
}
