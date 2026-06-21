//! Claim-on-commit: tie pending edit events to the commit that shipped
//! them, by **content** — never by session/commit assumptions.
//!
//! Algorithm (per committed file):
//! 1. **Exact blob match** — the committed blob equals some captured
//!    `post_blob`. The strongest possible evidence: the agent produced
//!    byte-identical content.
//! 2. **Hunk-level fallback** — partial staging (`git add -p`), hand
//!    formatting after the agent, or trailing edits mean no captured blob
//!    equals the committed one. Fall back to comparing *added lines*: the
//!    edit event whose added lines overlap the commit's added lines the
//!    most claims it (with `MatchKind::Hunk`, so readers know the
//!    confidence tier).
//! 3. **Ties break by causal order** — highest `(timestamp_us, seq)`
//!    wins: the last writer of identical content owns the outcome (the
//!    override chain itself is the provenance engine's job, P4).
//!
//! There is **no claim window**: the pending set is indexed by
//! `(path, blob)` and an edit stays claimable for as long as its session
//! state exists. Stash-pop or a commit made days later still claims by
//! content.

use std::collections::{HashMap, HashSet};
use std::process::{Command, Stdio};

use crate::hooks::state::ActiveSession;

/// A captured edit event eligible for claiming, flattened from session
/// state (origin chains + foreign-repo chains routed to this repo).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingEdit {
    pub session_id: String,
    pub agent: String,
    /// Repo-relative path.
    pub path: String,
    pub pre_blob: String,
    pub post_blob: String,
    pub seq: i64,
    pub timestamp_us: i64,
    /// Turn this edit belongs to, when known (snapshot edits always
    /// know; live-chain edits are the session's current turn).
    pub turn_index: Option<i64>,
    /// Editing tool (Write/Edit/StrReplace), when captured.
    pub tool_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    /// Committed blob == captured post_blob (byte-identical).
    ExactBlob,
    /// Added-line overlap (partial staging / trailing human edits).
    Hunk,
}

#[derive(Debug, Clone)]
pub struct Claim {
    pub session_id: String,
    pub agent: String,
    pub path: String,
    pub match_kind: MatchKind,
    /// How many of the commit's added lines this edit explains
    /// (exact matches explain all of them by definition).
    pub overlap_lines: usize,
}

#[derive(Debug, Default)]
pub struct ClaimResult {
    pub claims: Vec<Claim>,
    /// Files changed by the commit with no covering edit event —
    /// "no captured AI provenance", surfaced honestly.
    pub unclaimed_paths: Vec<String>,
}

impl ClaimResult {
    /// Session ids that claimed at least one file.
    pub fn claimed_sessions(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.claims
            .iter()
            .filter(|c| seen.insert(c.session_id.clone()))
            .map(|c| c.session_id.clone())
            .collect()
    }
}

/// Flatten claimable edit events for `repo_root` out of session state.
/// Includes both origin-session chains and foreign-repo chains that other
/// sessions routed INTO this repo (the cross-repo case).
pub fn collect_pending_edits(repo_root: &str, sessions: &[ActiveSession]) -> Vec<PendingEdit> {
    let canon_root = canon(repo_root);

    let mut out = Vec::new();
    for s in sessions {
        // Origin chain: counts when the session's worktree IS this repo
        // (or is unknown — legacy states recorded no worktree).
        let origin_matches = s
            .worktree
            .as_deref()
            .is_none_or(|wt| canon(wt) == canon_root);
        if origin_matches {
            if let Some(chain) = &s.file_edit_chain {
                push_chain(&mut out, s, chain);
            }
        }

        // Foreign chains explicitly routed to this repo.
        if let Some(foreign) = &s.foreign_repos {
            for (root, capture) in foreign {
                if canon(root) == canon_root {
                    if let Some(chain) = &capture.file_edit_chain {
                        push_chain(&mut out, s, chain);
                    }
                }
            }
        }
    }
    out
}

fn push_chain(
    out: &mut Vec<PendingEdit>,
    s: &ActiveSession,
    chain: &HashMap<String, Vec<crate::hooks::state::FileEditPair>>,
) {
    for (path, pairs) in chain {
        for pair in pairs {
            out.push(PendingEdit {
                session_id: s.session_id.clone(),
                agent: s.agent.clone(),
                path: path.clone(),
                pre_blob: pair.pre_blob.clone(),
                post_blob: pair.post_blob.clone(),
                seq: pair.seq,
                timestamp_us: pair.timestamp_us,
                turn_index: Some(s.current_turn_index),
                tool_name: pair.tool_name.clone(),
            });
        }
    }
}

/// Edit events preserved in git-backed turn snapshots. Live session
/// chains are drained into snapshots when a turn finishes, so committed
/// work usually claims from here; the live-chain collector covers
/// commits made mid-turn.
pub fn collect_snapshot_edits(snapshots: &[crate::core::turn::TurnSnapshot]) -> Vec<PendingEdit> {
    let mut out = Vec::new();
    for snap in snapshots {
        for f in &snap.files {
            let (Some(pre), Some(post)) = (&f.pre_blob, &f.post_blob) else {
                continue;
            };
            out.push(PendingEdit {
                session_id: snap.session_id.clone(),
                agent: snap.source.clone(),
                path: f.path.clone(),
                pre_blob: pre.clone(),
                post_blob: post.clone(),
                seq: snap.turn_index,
                timestamp_us: snap.ended_at.or(snap.started_at).unwrap_or(0),
                turn_index: Some(snap.turn_index),
                tool_name: None,
            });
        }
    }
    out
}

/// The full pending set for a repo: live session chains (origin +
/// foreign-routed) plus turn-snapshot edits. This is what
/// claim-on-commit runs against — no time window, content decides.
pub fn pending_for_repo(repo_root: &str) -> Vec<PendingEdit> {
    let sessions = crate::hooks::store::list_for_project(repo_root);
    let mut pending = collect_pending_edits(repo_root, &sessions);
    let snapshots = crate::git::turns::list_turn_snapshots(repo_root);
    pending.extend(collect_snapshot_edits(&snapshots));
    pending
}

/// A file changed by a commit, with git's authoritative blob ids.
#[derive(Debug, Clone)]
pub struct CommitFile {
    pub path: String,
    pub old_blob: String,
    pub new_blob: String,
}

/// Changed files of `sha` from `git diff-tree` raw output
/// (`--root` covers the initial commit).
pub fn commit_files(repo_root: &str, sha: &str) -> Vec<CommitFile> {
    let Some(raw) = git(
        repo_root,
        &["diff-tree", "-r", "--root", "--no-commit-id", sha],
    ) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|line| {
            // :100644 100644 <old> <new> M\tpath
            let line = line.strip_prefix(':')?;
            let (meta, path) = line.split_once('\t')?;
            let fields: Vec<&str> = meta.split_whitespace().collect();
            if fields.len() < 5 {
                return None;
            }
            Some(CommitFile {
                path: path.to_string(),
                old_blob: fields[2].to_string(),
                new_blob: fields[3].to_string(),
            })
        })
        .collect()
}

/// Run the claim for one commit against the pending set.
pub fn claim_commit(repo_root: &str, sha: &str, pending: &[PendingEdit]) -> ClaimResult {
    let changed = commit_files(repo_root, sha);
    let mut by_path: HashMap<&str, Vec<&PendingEdit>> = HashMap::new();
    for edit in pending {
        by_path.entry(edit.path.as_str()).or_default().push(edit);
    }

    let mut result = ClaimResult::default();
    for file in &changed {
        if is_null_blob(&file.new_blob) {
            // Deletion: nothing to attribute content-wise here.
            continue;
        }
        let Some(candidates) = by_path.get(file.path.as_str()) else {
            result.unclaimed_paths.push(file.path.clone());
            continue;
        };

        // Tier 1: exact blob match; last writer (causal order) wins.
        if let Some(winner) = candidates
            .iter()
            .filter(|e| blob_matches(&e.post_blob, &file.new_blob))
            .max_by_key(|e| (e.timestamp_us, e.seq))
        {
            let committed_added = added_lines(repo_root, &file.old_blob, &file.new_blob);
            result.claims.push(Claim {
                session_id: winner.session_id.clone(),
                agent: winner.agent.clone(),
                path: file.path.clone(),
                match_kind: MatchKind::ExactBlob,
                overlap_lines: committed_added.len(),
            });
            continue;
        }

        // Tier 2: hunk fallback by added-line overlap.
        let committed_added = added_lines(repo_root, &file.old_blob, &file.new_blob);
        if committed_added.is_empty() {
            result.unclaimed_paths.push(file.path.clone());
            continue;
        }
        let best = candidates
            .iter()
            .map(|e| {
                let edit_added = added_lines(repo_root, &e.pre_blob, &e.post_blob);
                let overlap = committed_added.intersection(&edit_added).count();
                (overlap, e)
            })
            .filter(|(overlap, _)| *overlap > 0)
            .max_by_key(|(overlap, e)| (*overlap, e.timestamp_us, e.seq));

        match best {
            Some((overlap, winner)) => result.claims.push(Claim {
                session_id: winner.session_id.clone(),
                agent: winner.agent.clone(),
                path: file.path.clone(),
                match_kind: MatchKind::Hunk,
                overlap_lines: overlap,
            }),
            None => result.unclaimed_paths.push(file.path.clone()),
        }
    }
    result
}

fn canon(path: &str) -> String {
    std::fs::canonicalize(path).map_or_else(
        |_| path.to_string(),
        |p| crate::utils::normalize_win_path(&p.to_string_lossy()).to_string(),
    )
}

fn is_null_blob(sha: &str) -> bool {
    sha.is_empty() || sha.bytes().all(|b| b == b'0')
}

/// Captured blobs may be abbreviated; compare by prefix against git's
/// full object id (minimum 7 chars to avoid degenerate prefixes).
fn blob_matches(captured: &str, committed: &str) -> bool {
    if captured.is_empty() || captured.len() < 7 {
        return false;
    }
    committed.starts_with(captured) || captured.starts_with(committed)
}

/// Non-blank added lines between two blobs. Empty/null `pre` means the
/// file was created: every non-blank line counts as added.
fn added_lines(repo_root: &str, pre: &str, post: &str) -> HashSet<String> {
    if is_null_blob(post) {
        return HashSet::new();
    }
    if is_null_blob(pre) {
        let Some(content) = git(repo_root, &["cat-file", "-p", post]) else {
            return HashSet::new();
        };
        return content
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty())
            .map(ToString::to_string)
            .collect();
    }
    let Some(diff) = git(repo_root, &["diff", "--no-color", pre, post]) else {
        return HashSet::new();
    };
    diff.lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .map(|l| l[1..].trim_end())
        .filter(|l| !l.trim().is_empty())
        .map(ToString::to_string)
        .collect()
}

fn git(repo_root: &str, args: &[&str]) -> Option<String> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let mut cmd = Command::new(git);
    cmd.args(args)
        .current_dir(repo_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::git::proxy::scrub_git_env(&mut cmd);
    let output = cmd.output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Repo {
        _tmp: tempfile::TempDir,
        root: String,
    }

    fn init_repo() -> Option<Repo> {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap().to_string();
        let ok = Command::new("git")
            .args(["init", &root])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            return None;
        }
        for args in [
            &["config", "user.name", "T"][..],
            &["config", "user.email", "t@t"][..],
        ] {
            let _ = Command::new("git").arg("-C").arg(&root).args(args).output();
        }
        Some(Repo { _tmp: tmp, root })
    }

    fn hash_blob(root: &str, content: &str) -> String {
        use std::io::Write;
        let mut child = Command::new("git")
            .args(["-C", root, "hash-object", "-w", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(content.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit_file(root: &str, path: &str, content: &str, msg: &str) -> String {
        std::fs::write(format!("{root}/{path}"), content).unwrap();
        Command::new("git")
            .args(["-C", root, "add", path])
            .output()
            .unwrap();
        Command::new("git")
            .args(["-C", root, "commit", "-m", msg])
            .output()
            .unwrap();
        let out = Command::new("git")
            .args(["-C", root, "rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn edit(session: &str, path: &str, pre: &str, post: &str, seq: i64) -> PendingEdit {
        PendingEdit {
            session_id: session.into(),
            agent: "claude".into(),
            path: path.into(),
            pre_blob: pre.into(),
            post_blob: post.into(),
            seq,
            timestamp_us: seq * 1000,
            turn_index: None,
            tool_name: None,
        }
    }

    #[test]
    fn exact_blob_match_claims_file() {
        let Some(repo) = init_repo() else { return };
        let root = &repo.root;

        let v1 = "fn a() {}\n";
        let v2 = "fn a() {}\nfn b() {}\n";
        let pre = hash_blob(root, v1);
        let post = hash_blob(root, v2);
        commit_file(root, "a.rs", v1, "base");
        let sha = commit_file(root, "a.rs", v2, "agent edit");

        let pending = vec![edit("s1", "a.rs", &pre, &post, 1)];
        let result = claim_commit(root, &sha, &pending);

        assert_eq!(result.claims.len(), 1);
        assert_eq!(result.claims[0].session_id, "s1");
        assert_eq!(result.claims[0].match_kind, MatchKind::ExactBlob);
        assert!(result.unclaimed_paths.is_empty());
    }

    #[test]
    fn last_writer_wins_exact_tie() {
        let Some(repo) = init_repo() else { return };
        let root = &repo.root;

        let v1 = "x\n";
        let v2 = "x\ny\n";
        let pre = hash_blob(root, v1);
        let post = hash_blob(root, v2);
        commit_file(root, "f.txt", v1, "base");
        let sha = commit_file(root, "f.txt", v2, "edit");

        // Two sessions produced byte-identical content; later one claims.
        let pending = vec![
            edit("early", "f.txt", &pre, &post, 1),
            edit("late", "f.txt", &pre, &post, 9),
        ];
        let result = claim_commit(root, &sha, &pending);
        assert_eq!(result.claims.len(), 1);
        assert_eq!(result.claims[0].session_id, "late");
    }

    #[test]
    fn partial_staging_claims_via_hunk_fallback() {
        let Some(repo) = init_repo() else { return };
        let root = &repo.root;

        let base = "line1\n";
        commit_file(root, "f.txt", base, "base");

        // Agent produced base + A + B, but the user staged only base + A
        // (partial staging) — no captured blob equals the committed one.
        let agent_full = "line1\nadded-by-agent-A\nadded-by-agent-B\n";
        let committed = "line1\nadded-by-agent-A\n";
        let pre = hash_blob(root, base);
        let post = hash_blob(root, agent_full);
        let sha = commit_file(root, "f.txt", committed, "partial stage");

        let pending = vec![edit("s1", "f.txt", &pre, &post, 1)];
        let result = claim_commit(root, &sha, &pending);

        assert_eq!(result.claims.len(), 1);
        assert_eq!(result.claims[0].match_kind, MatchKind::Hunk);
        assert_eq!(result.claims[0].overlap_lines, 1);
    }

    #[test]
    fn hunk_tie_breaks_by_overlap_then_causal_order() {
        let Some(repo) = init_repo() else { return };
        let root = &repo.root;

        let base = "base\n";
        commit_file(root, "f.txt", base, "base");
        let pre = hash_blob(root, base);

        // Session A added two of the committed lines, session B one.
        let a_post = hash_blob(root, "base\nalpha\nbeta\n");
        let b_post = hash_blob(root, "base\nalpha\n");
        let sha = commit_file(root, "f.txt", "base\nalpha\nbeta\nhuman\n", "mix");

        let pending = vec![
            edit("a", "f.txt", &pre, &a_post, 1),
            edit("b", "f.txt", &pre, &b_post, 2),
        ];
        let result = claim_commit(root, &sha, &pending);
        assert_eq!(result.claims.len(), 1);
        assert_eq!(result.claims[0].session_id, "a", "higher overlap wins");
        assert_eq!(result.claims[0].overlap_lines, 2);
    }

    #[test]
    fn untouched_file_is_reported_unclaimed() {
        let Some(repo) = init_repo() else { return };
        let root = &repo.root;

        commit_file(root, "f.txt", "v1\n", "base");
        let sha = commit_file(root, "f.txt", "v2\n", "hand edit");

        let result = claim_commit(root, &sha, &[]);
        assert!(result.claims.is_empty());
        assert_eq!(result.unclaimed_paths, vec!["f.txt".to_string()]);
    }

    #[test]
    fn new_file_creation_claims_exactly() {
        let Some(repo) = init_repo() else { return };
        let root = &repo.root;

        commit_file(root, "seed.txt", "seed\n", "seed");
        let content = "brand new file\n";
        let post = hash_blob(root, content);
        let sha = commit_file(root, "new.txt", content, "create");

        let pending = vec![edit("s1", "new.txt", "", &post, 1)];
        let result = claim_commit(root, &sha, &pending);
        assert_eq!(result.claims.len(), 1);
        assert_eq!(result.claims[0].match_kind, MatchKind::ExactBlob);
    }

    #[test]
    fn stale_claim_no_window_still_claims_by_content() {
        let Some(repo) = init_repo() else { return };
        let root = &repo.root;

        // Edit captured "days ago" (timestamp irrelevant — content rules).
        let v1 = "a\n";
        let v2 = "a\nb\n";
        let pre = hash_blob(root, v1);
        let post = hash_blob(root, v2);
        commit_file(root, "f.txt", v1, "base");
        let sha = commit_file(root, "f.txt", v2, "late commit");

        let mut old_edit = edit("s1", "f.txt", &pre, &post, 1);
        old_edit.timestamp_us = 1; // epoch-old
        let result = claim_commit(root, &sha, &[old_edit]);
        assert_eq!(result.claims.len(), 1, "no claim window: content decides");
    }

    /// Cherry-pick: the new commit has a different sha but the same
    /// content blobs — claim-by-content survives untouched. (Rebase is
    /// the same mechanism: replayed commits, identical blobs.)
    #[test]
    fn cherry_pick_claims_by_content_on_new_sha() {
        let Some(repo) = init_repo() else { return };
        let root = &repo.root;

        commit_file(root, "f.txt", "base\n", "base");
        let base_sha = git_out(root, &["rev-parse", "HEAD"]);

        // Agent edit committed on a side branch.
        let pre = hash_blob(root, "base\n");
        let post = hash_blob(root, "base\nagent-line\n");
        Command::new("git")
            .args(["-C", root, "checkout", "-b", "side"])
            .output()
            .unwrap();
        let side_sha = commit_file(root, "f.txt", "base\nagent-line\n", "agent edit");

        // Cherry-pick onto a *diverged* branch → brand-new sha.
        Command::new("git")
            .args(["-C", root, "checkout", "-b", "target", &base_sha])
            .output()
            .unwrap();
        commit_file(root, "other.txt", "divergence\n", "diverge");
        Command::new("git")
            .args(["-C", root, "cherry-pick", &side_sha])
            .output()
            .unwrap();
        let picked_sha = git_out(root, &["rev-parse", "HEAD"]);
        assert_ne!(picked_sha, side_sha, "cherry-pick must create a new sha");

        let pending = vec![edit("s1", "f.txt", &pre, &post, 1)];
        let result = claim_commit(root, &picked_sha, &pending);
        assert_eq!(result.claims.len(), 1, "content survives the rewrite");
        assert_eq!(result.claims[0].match_kind, MatchKind::ExactBlob);
    }

    /// Revert commits only delete the agent's lines — nothing to claim,
    /// nothing to crash on (the deletion path).
    #[test]
    fn revert_commit_claims_nothing_cleanly() {
        let Some(repo) = init_repo() else { return };
        let root = &repo.root;

        commit_file(root, "f.txt", "base\n", "base");
        let agent_sha = commit_file(root, "f.txt", "base\nagent\n", "agent edit");
        Command::new("git")
            .args(["-C", root, "revert", "--no-edit", &agent_sha])
            .output()
            .unwrap();
        let revert_sha = git_out(root, &["rev-parse", "HEAD"]);
        assert_ne!(revert_sha, agent_sha);

        let pre = hash_blob(root, "base\n");
        let post = hash_blob(root, "base\nagent\n");
        let pending = vec![edit("s1", "f.txt", &pre, &post, 1)];
        let result = claim_commit(root, &revert_sha, &pending);
        assert!(
            result.claims.is_empty(),
            "a revert adds none of the agent's content"
        );
    }

    /// merge --squash: one commit carries the union of edits from two
    /// sessions on different files — each file claims to its session.
    #[test]
    fn squash_merge_claims_union_of_constituent_edits() {
        let Some(repo) = init_repo() else { return };
        let root = &repo.root;

        commit_file(root, "seed.txt", "seed\n", "seed");
        let base_sha = git_out(root, &["rev-parse", "HEAD"]);

        // Branch with two commits: session A edits a.txt, session B b.txt.
        Command::new("git")
            .args(["-C", root, "checkout", "-b", "feature"])
            .output()
            .unwrap();
        let a_post = hash_blob(root, "from-A\n");
        commit_file(root, "a.txt", "from-A\n", "A's work");
        let b_post = hash_blob(root, "from-B\n");
        commit_file(root, "b.txt", "from-B\n", "B's work");

        // Squash-merge into a target branch: ONE new commit, both files.
        Command::new("git")
            .args(["-C", root, "checkout", "-b", "main2", &base_sha])
            .output()
            .unwrap();
        Command::new("git")
            .args(["-C", root, "merge", "--squash", "feature"])
            .output()
            .unwrap();
        Command::new("git")
            .args(["-C", root, "commit", "-m", "squashed"])
            .output()
            .unwrap();
        let squash_sha = git_out(root, &["rev-parse", "HEAD"]);

        let pending = vec![
            edit("session-a", "a.txt", "", &a_post, 1),
            edit("session-b", "b.txt", "", &b_post, 2),
        ];
        let result = claim_commit(root, &squash_sha, &pending);
        assert_eq!(result.claims.len(), 2, "claims: {:?}", result.claims);
        let by_path: HashMap<&str, &str> = result
            .claims
            .iter()
            .map(|c| (c.path.as_str(), c.session_id.as_str()))
            .collect();
        assert_eq!(by_path["a.txt"], "session-a");
        assert_eq!(by_path["b.txt"], "session-b");
    }

    fn git_out(root: &str, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn collect_pending_edits_includes_foreign_chains() {
        let Some(repo) = init_repo() else { return };
        let root = &repo.root;

        let mut chain = HashMap::new();
        chain.insert(
            "src/x.rs".to_string(),
            vec![crate::hooks::state::FileEditPair {
                pre_blob: "aaa1111".into(),
                post_blob: "bbb2222".into(),
                tool_name: Some("Edit".into()),
                timestamp: 1,
                seq: 1,
                timestamp_us: 1,
            }],
        );

        // Session whose ORIGIN is elsewhere but which routed edits here.
        let mut foreign = HashMap::new();
        foreign.insert(
            root.clone(),
            crate::hooks::state::RepoCapture {
                file_edit_chain: Some(chain),
                ..Default::default()
            },
        );
        let session = crate::hooks::state::test_support::mk_session_with(
            "s-foreign",
            "/somewhere/else",
            None,
            Some(foreign),
        );

        let edits = collect_pending_edits(root, &[session]);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].path, "src/x.rs");
        assert_eq!(edits[0].session_id, "s-foreign");

        // Origin session in THIS repo contributes its chain too.
        let mut origin_chain = HashMap::new();
        origin_chain.insert(
            "y.rs".to_string(),
            vec![crate::hooks::state::FileEditPair {
                pre_blob: "ccc3333".into(),
                post_blob: "ddd4444".into(),
                tool_name: None,
                timestamp: 2,
                seq: 2,
                timestamp_us: 2,
            }],
        );
        let origin = crate::hooks::state::test_support::mk_session_with(
            "s-origin",
            root,
            Some(origin_chain),
            None,
        );
        let edits = collect_pending_edits(root, &[origin]);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].session_id, "s-origin");
    }
}
