//! Provenance engine — blame++ (line → edit → turn → trigger → session).
//!
//! Reconstructs a file's **micro-history** for one commit from captured
//! edit events, then replays it with *metadata patching*: an attribution
//! vector (one origin per line) is transformed through every diff in the
//! chain, so each committed line maps to the exact edit that last
//! produced it — position-accurate, content-proven, independent of
//! session/commit assumptions.
//!
//! The replay is honest about what it doesn't know:
//! - **Uncaptured windows** — the file state jumped without a covering
//!   edit event (hand edits, another tool without hooks). Lines from
//!   those windows are `LineOrigin::Uncaptured`, bounded by the steps
//!   around them. "No captured AI provenance", never "human" by fiat.
//! - **Clobber records** — work that was overwritten before it ever
//!   landed (A wrote, B replaced). The override chain is kept, not
//!   collapsed: every step stays in `steps` with its survival count.
//! - **Disconnected chains** — a step whose `pre_blob` doesn't extend
//!   the previous state is marked `linked = false` (ambiguity marker).
//!
//! The core (`compute_file_provenance`) is a pure function over plain
//! data plus a blob reader, so the whole engine is testable without
//! sessions, hooks, or a live agent. Gathering (v2 store + live session
//! state) lives in [`gather`]; per-commit caching in [`cache`].

pub mod cache;
pub mod gather;

use serde::{Deserialize, Serialize};

/// Why a turn ran — classification of the actor behind an edit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Trigger {
    /// A human prompt directly preceded this turn (prompt preview kept,
    /// already sanitized by the publish choke-point).
    HumanDirected {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
    },
    /// A subagent made the edit; the parent turn carries the intent.
    Subagent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_type: Option<String>,
    },
    /// The agent acted within its loop without a fresh human prompt.
    AgentAutonomous,
    /// Not enough captured context to classify.
    Unknown,
}

/// One captured edit in a file's micro-history (input to the replay).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditInput {
    pub session_id: String,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_index: Option<i64>,
    pub seq: i64,
    pub timestamp_us: i64,
    pub pre_blob: String,
    pub post_blob: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    pub trigger: Trigger,
}

/// A replayed edit step with survival accounting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditStep {
    #[serde(flatten)]
    pub edit: EditInput,
    /// Causal-chain integrity: `pre_blob` extended the previous state.
    /// `false` = disconnected chain (ambiguity marker).
    pub linked: bool,
    /// Lines this step added during replay.
    pub lines_added: u32,
    /// How many of those lines survive in the committed file.
    pub lines_surviving: u32,
}

/// A window where the file changed with no covering edit event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UncapturedWindow {
    /// Step index the window follows (`None` = before the first capture).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_step: Option<usize>,
    /// Step index the window precedes (`None` = trailing window between
    /// the last captured state and the committed blob).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_step: Option<usize>,
    /// Lines introduced inside this window.
    pub lines_added: u32,
}

/// Captured work that was overwritten before it landed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClobberRecord {
    /// The step whose lines were lost.
    pub victim_step: usize,
    /// The step that overwrote them (`None` = lost to an uncaptured
    /// window — e.g. a hand edit reverted the agent's work).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_step: Option<usize>,
    pub lines_lost: u32,
}

/// Where one committed line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LineOrigin {
    /// Existed at the baseline (parent commit) — outside this window.
    Baseline,
    /// Produced by `steps[idx]`.
    Edit { step: usize },
    /// Introduced inside `gaps[idx]` — no captured provenance.
    Uncaptured { gap: usize },
}

/// Full provenance of one file at one commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileProvenance {
    pub schema_version: u32,
    pub commit: String,
    pub path: String,
    pub baseline_blob: String,
    pub committed_blob: String,
    /// The micro-history, causally ordered. Clobbered steps stay here —
    /// the override chain is part of the record.
    pub steps: Vec<EditStep>,
    pub gaps: Vec<UncapturedWindow>,
    pub clobbers: Vec<ClobberRecord>,
    /// One origin per committed line (index 0 = line 1).
    pub lines: Vec<LineOrigin>,
}

pub const PROVENANCE_SCHEMA_VERSION: u32 = 1;

impl FileProvenance {
    /// Origin of a 1-indexed line.
    pub fn origin_of(&self, lineno: u32) -> Option<&LineOrigin> {
        self.lines.get(lineno.saturating_sub(1) as usize)
    }

    /// The step behind a 1-indexed line, if it has captured provenance.
    pub fn step_for_line(&self, lineno: u32) -> Option<&EditStep> {
        match self.origin_of(lineno)? {
            LineOrigin::Edit { step } => self.steps.get(*step),
            _ => None,
        }
    }
}

/// Blob access needed by the replay: read content and diff two blobs.
/// Abstracted so the engine itself stays a pure function (tests inject
/// an in-memory implementation; production shells out to git).
pub trait BlobSource {
    /// Full content of a blob ("" for the null/empty blob).
    fn read(&self, blob: &str) -> Option<String>;
}

/// Git-backed blob source for a repo.
pub struct GitBlobSource<'a> {
    pub repo_root: &'a str,
}

impl BlobSource for GitBlobSource<'_> {
    fn read(&self, blob: &str) -> Option<String> {
        if is_null_blob(blob) {
            return Some(String::new());
        }
        let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
        let output = std::process::Command::new(git)
            .args(["cat-file", "-p", blob])
            .current_dir(self.repo_root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            None
        }
    }
}

pub fn is_null_blob(sha: &str) -> bool {
    sha.is_empty() || sha.bytes().all(|b| b == b'0')
}

/// Abbreviation-tolerant blob identity (captured hashes may be short).
fn blob_eq(a: &str, b: &str) -> bool {
    if is_null_blob(a) || is_null_blob(b) {
        return is_null_blob(a) && is_null_blob(b);
    }
    if a.len() < 7 || b.len() < 7 {
        return a == b;
    }
    a.starts_with(b) || b.starts_with(a)
}

/// Reconstruct provenance for one file at one commit.
///
/// `edits` is every captured edit event for this path (any session, any
/// order); the engine orders them causally and replays.
pub fn compute_file_provenance(
    blobs: &dyn BlobSource,
    commit: &str,
    path: &str,
    baseline_blob: &str,
    committed_blob: &str,
    mut edits: Vec<EditInput>,
) -> FileProvenance {
    // Causal order: timestamp, then per-session sequence. Hash-chain
    // linkage is *verified* during replay (linked flag), not trusted
    // blindly — wall clocks lie, content doesn't.
    edits.sort_by_key(|e| (e.timestamp_us, e.seq));
    // Drop no-op events (pre == post): they add nothing and would
    // pollute the chain.
    edits.retain(|e| !blob_eq(&e.pre_blob, &e.post_blob));

    let mut steps: Vec<EditStep> = Vec::new();
    let mut gaps: Vec<UncapturedWindow> = Vec::new();
    let mut clobber_acc: std::collections::HashMap<(usize, Option<usize>), u32> =
        std::collections::HashMap::new();

    let baseline_content = blobs.read(baseline_blob).unwrap_or_default();
    let mut attr: Vec<LineOrigin> = vec![LineOrigin::Baseline; count_lines(&baseline_content)];
    let mut state_blob = baseline_blob.to_string();
    let mut state_content = baseline_content;

    for edit in edits {
        let step_idx = steps.len();
        let linked = blob_eq(&edit.pre_blob, &state_blob);

        // Disconnected chain: the file moved from our last known state
        // to this edit's pre-state without capture. Surface the window
        // and advance through it so the replay coordinates stay true.
        if !linked {
            let Some(pre_content) = blobs.read(&edit.pre_blob) else {
                // Pre-blob unreadable (e.g. GC'd): keep the step,
                // unlinked, and replay current-state → post directly.
                // A zero-line gap marker records the discontinuity.
                let Some(post_content) = blobs.read(&edit.post_blob) else {
                    continue;
                };
                gaps.push(UncapturedWindow {
                    after_step: step_idx.checked_sub(1),
                    before_step: Some(step_idx),
                    lines_added: 0,
                });
                let (new_attr, added, _) = apply_diff(
                    &state_content,
                    &post_content,
                    &attr,
                    LineOrigin::Edit { step: step_idx },
                    &mut clobber_acc,
                    Some(step_idx),
                );
                attr = new_attr;
                state_blob.clone_from(&edit.post_blob);
                state_content = post_content;
                steps.push(EditStep {
                    edit,
                    linked: false,
                    lines_added: added,
                    lines_surviving: 0,
                });
                continue;
            };

            let gap_idx = gaps.len();
            let (new_attr, gap_added, _) = apply_diff(
                &state_content,
                &pre_content,
                &attr,
                LineOrigin::Uncaptured { gap: gap_idx },
                &mut clobber_acc,
                None,
            );
            attr = new_attr;
            gaps.push(UncapturedWindow {
                after_step: step_idx.checked_sub(1),
                before_step: Some(step_idx),
                lines_added: gap_added,
            });
            state_blob.clone_from(&edit.pre_blob);
            state_content = pre_content;
        }

        let Some(post_content) = blobs.read(&edit.post_blob) else {
            continue;
        };
        let (new_attr, added, _) = apply_diff(
            &state_content,
            &post_content,
            &attr,
            LineOrigin::Edit { step: step_idx },
            &mut clobber_acc,
            Some(step_idx),
        );
        attr = new_attr;
        state_blob.clone_from(&edit.post_blob);
        state_content = post_content;
        steps.push(EditStep {
            edit,
            linked,
            lines_added: added,
            lines_surviving: 0,
        });
    }

    // Trailing window: last captured state → committed blob (hand edits
    // after the agent, partial staging).
    let committed_content = blobs.read(committed_blob).unwrap_or_default();
    if !blob_eq(&state_blob, committed_blob) && state_content != committed_content {
        let gap_idx = gaps.len();
        let (new_attr, gap_added, _) = apply_diff(
            &state_content,
            &committed_content,
            &attr,
            LineOrigin::Uncaptured { gap: gap_idx },
            &mut clobber_acc,
            None,
        );
        attr = new_attr;
        gaps.push(UncapturedWindow {
            after_step: steps.len().checked_sub(1),
            before_step: None,
            lines_added: gap_added,
        });
    }

    // Survival accounting + clobber records.
    for origin in &attr {
        if let LineOrigin::Edit { step } = origin {
            if let Some(s) = steps.get_mut(*step) {
                s.lines_surviving += 1;
            }
        }
    }
    let mut clobbers: Vec<ClobberRecord> = clobber_acc
        .into_iter()
        .map(|((victim_step, by_step), lines_lost)| ClobberRecord {
            victim_step,
            by_step,
            lines_lost,
        })
        .collect();
    clobbers.sort_by_key(|c| (c.victim_step, c.by_step));

    FileProvenance {
        schema_version: PROVENANCE_SCHEMA_VERSION,
        commit: commit.to_string(),
        path: path.to_string(),
        baseline_blob: baseline_blob.to_string(),
        committed_blob: committed_blob.to_string(),
        steps,
        gaps,
        clobbers,
        lines: attr,
    }
}

fn count_lines(content: &str) -> usize {
    content.lines().count()
}

/// Transform the attribution vector through one content transition
/// (a → b): unchanged lines keep their origins, removed lines record
/// clobbers against `by_step`, added lines take `new_origin`.
///
/// Returns `(new_attr, lines_added, lines_removed)`.
fn apply_diff(
    a: &str,
    b: &str,
    attr: &[LineOrigin],
    new_origin: LineOrigin,
    clobbers: &mut std::collections::HashMap<(usize, Option<usize>), u32>,
    by_step: Option<usize>,
) -> (Vec<LineOrigin>, u32, u32) {
    let hunks = diff_hunks(a, b);
    let mut new_attr: Vec<LineOrigin> = Vec::with_capacity(b.lines().count());
    let mut old_pos: usize = 1; // 1-indexed cursor into `attr`
    let mut added: u32 = 0;
    let mut removed: u32 = 0;

    for h in &hunks {
        // With -U0 semantics: old_count == 0 means "insert AFTER line
        // old_start" (copy through old_start); otherwise lines
        // [old_start, old_start+old_count) are replaced.
        let copy_until = if h.old_count == 0 {
            h.old_start
        } else {
            h.old_start.saturating_sub(1)
        };
        while old_pos <= copy_until as usize && old_pos <= attr.len() {
            new_attr.push(attr[old_pos - 1]);
            old_pos += 1;
        }
        for _ in 0..h.old_count {
            if old_pos <= attr.len() {
                if let LineOrigin::Edit { step } = attr[old_pos - 1] {
                    if by_step != Some(step) {
                        *clobbers.entry((step, by_step)).or_insert(0) += 1;
                    }
                }
                removed += 1;
                old_pos += 1;
            }
        }
        for _ in 0..h.new_count {
            new_attr.push(new_origin);
            added += 1;
        }
    }
    while old_pos <= attr.len() {
        new_attr.push(attr[old_pos - 1]);
        old_pos += 1;
    }

    (new_attr, added, removed)
}

#[derive(Debug, PartialEq)]
struct Hunk {
    old_start: u32,
    old_count: u32,
    new_start: u32,
    new_count: u32,
}

/// Minimal line-diff producing -U0-style hunks, computed in-process (no
/// subprocess per chain step). Histogram-free LCS via the classic O(ND)
/// Myers algorithm on line hashes — files in edit chains are small
/// enough that simplicity wins.
fn diff_hunks(old_text: &str, new_text: &str) -> Vec<Hunk> {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let new_lines: Vec<&str> = new_text.lines().collect();

    // LCS table via dynamic programming on the line vectors. Edit-chain
    // blobs are file-sized (not repo-sized); O(n·m) here is fine and
    // entirely deterministic.
    let old_len = old_lines.len();
    let new_len = new_lines.len();
    if old_len == 0 && new_len == 0 {
        return Vec::new();
    }
    if old_len == 0 {
        return vec![Hunk {
            old_start: 0,
            old_count: 0,
            new_start: 1,
            new_count: new_len as u32,
        }];
    }
    if new_len == 0 {
        return vec![Hunk {
            old_start: 1,
            old_count: old_len as u32,
            new_start: 0,
            new_count: 0,
        }];
    }

    let mut lcs = vec![vec![0u32; new_len + 1]; old_len + 1];
    for i in (0..old_len).rev() {
        for j in (0..new_len).rev() {
            lcs[i][j] = if old_lines[i] == new_lines[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    // Walk the table emitting maximal replace/insert/delete hunks:
    // on entering a mismatch region remember (hi, hj); on exiting,
    // deletes = i - hi and inserts = j - hj.
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);

    while i < old_len || j < new_len {
        if i < old_len && j < new_len && old_lines[i] == new_lines[j] {
            i += 1;
            j += 1;
            continue;
        }
        let (hi, hj) = (i, j);
        while i < old_len || j < new_len {
            if i < old_len && j < new_len && old_lines[i] == new_lines[j] {
                break;
            }
            if j < new_len && (i >= old_len || lcs[i][j + 1] >= lcs[i + 1][j]) {
                j += 1;
            } else {
                i += 1;
            }
        }
        let deletes = (i - hi) as u32;
        let inserts = (j - hj) as u32;
        hunks.push(Hunk {
            // -U0 convention: count 0 → "after line N" anchor.
            old_start: if deletes > 0 {
                hi as u32 + 1
            } else {
                hi as u32
            },
            old_count: deletes,
            new_start: if inserts > 0 {
                hj as u32 + 1
            } else {
                hj as u32
            },
            new_count: inserts,
        });
    }

    hunks
}

#[cfg(test)]
mod tests;
