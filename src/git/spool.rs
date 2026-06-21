//! Commit spool — the zero-interference write path.
//!
//! The git write path (post-commit hook, proxy intercept) must cost
//! effectively nothing. Instead of enriching synchronously, it appends a
//! one-line JSON entry to an append-only spool file in the git common
//! dir and returns. A detached worker drains the spool asynchronously.
//!
//! Crash-safety contract:
//! - An entry survives in the spool until its enrichment is **durably
//!   written**. The worker first atomically renames `spool.jsonl` to a
//!   uniquely-named `*.work` file (new appends create a fresh spool), then
//!   processes the work file, and deletes it only when every entry
//!   succeeded. Failed entries are rewritten into the work file.
//! - A crashed worker leaves `*.work` files behind; the next drain picks
//!   them up. Processing is idempotent, so replays are no-ops.
//!
//! Speed contract: [`append_commit`] resolves the repo layout and HEAD by
//! reading files (`.git`, `HEAD`, loose refs, `packed-refs`) — **no
//! subprocesses** on the happy path. A single `O_APPEND` write lands the
//! entry; small writes to a local file are atomic in practice.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::CliError;

/// One pending enrichment: a commit that needs anchor/provenance work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpoolEntry {
    /// Worktree root the commit was made in.
    pub root: String,
    pub sha: String,
    #[serde(default)]
    pub branch: String,
    /// Capture timestamp (epoch seconds).
    #[serde(default)]
    pub ts: i64,
}

/// Directory holding the spool, work files and the worker lock:
/// `<git common dir>/oobo/`.
pub fn spool_dir(project_root: &str) -> Option<PathBuf> {
    let common = fast_git_common_dir(Path::new(project_root))
        .unwrap_or_else(|| crate::git::detect::resolve_git_common_dir(project_root));
    if common.as_os_str().is_empty() {
        return None;
    }
    Some(common.join("oobo"))
}

fn spool_file(dir: &Path) -> PathBuf {
    dir.join("spool.jsonl")
}

/// Append the current HEAD commit of `project_root` to the spool.
/// Subprocess-free on the happy path; falls back to git plumbing only
/// when the repo layout defeats the file-based resolver.
pub fn append_commit(project_root: &str) -> Result<SpoolEntry, CliError> {
    let (sha, branch) = head_state(project_root)
        .ok_or_else(|| CliError::Git("could not resolve HEAD for spool".into()))?;
    let entry = SpoolEntry {
        root: project_root.to_string(),
        sha,
        branch,
        ts: chrono::Utc::now().timestamp(),
    };
    append_entry(project_root, &entry)?;
    Ok(entry)
}

/// Append a pre-built entry (used when the caller already knows the sha).
pub fn append_entry(project_root: &str, entry: &SpoolEntry) -> Result<(), CliError> {
    let dir = spool_dir(project_root)
        .ok_or_else(|| CliError::Git("could not resolve git common dir for spool".into()))?;
    fs::create_dir_all(&dir).map_err(|e| CliError::Git(format!("spool dir: {e}")))?;

    let mut line = serde_json::to_string(entry).map_err(|e| CliError::Git(e.to_string()))?;
    line.push('\n');

    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(spool_file(&dir))
        .map_err(|e| CliError::Git(format!("spool open: {e}")))?;
    f.write_all(line.as_bytes())
        .map_err(|e| CliError::Git(format!("spool write: {e}")))?;
    Ok(())
}

/// True when there is pending spool work (entries or leftover work files).
pub fn has_pending(project_root: &str) -> bool {
    let Some(dir) = spool_dir(project_root) else {
        return false;
    };
    if spool_file(&dir).exists() {
        return true;
    }
    work_files(&dir).next().is_some()
}

fn work_files(dir: &Path) -> impl Iterator<Item = PathBuf> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "work"))
}

/// Claim all pending work: atomically renames the live spool to a unique
/// work file (concurrent appenders either land in the renamed inode —
/// still processed — or create a fresh spool), then returns every work
/// file present, including leftovers from crashed workers.
pub fn take_work(project_root: &str) -> Vec<PathBuf> {
    let Some(dir) = spool_dir(project_root) else {
        return Vec::new();
    };
    let live = spool_file(&dir);
    if live.exists() {
        let unique = format!(
            "spool-{}-{}.work",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let _ = fs::rename(&live, dir.join(unique));
    }
    let mut files: Vec<PathBuf> = work_files(&dir).collect();
    files.sort();
    files
}

/// Parse a work file into entries, deduplicated by `(root, sha)` keeping
/// the first occurrence. Malformed lines are dropped (and logged).
pub fn read_entries(work_file: &Path) -> Vec<SpoolEntry> {
    let Ok(content) = fs::read_to_string(work_file) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<SpoolEntry>(line) {
            Ok(e) => {
                if seen.insert((e.root.clone(), e.sha.clone())) {
                    out.push(e);
                }
            }
            Err(err) => tracing::warn!(%err, line, "dropping malformed spool line"),
        }
    }
    out
}

/// Mark a work file done: delete it when everything succeeded, otherwise
/// rewrite it with only the failed entries (atomic tmp+rename) so they
/// are retried on the next drain.
pub fn complete_work(work_file: &Path, failed: &[SpoolEntry]) {
    if failed.is_empty() {
        let _ = fs::remove_file(work_file);
        return;
    }
    let mut content = String::new();
    for e in failed {
        if let Ok(line) = serde_json::to_string(e) {
            content.push_str(&line);
            content.push('\n');
        }
    }
    let tmp = work_file.with_extension("work.tmp");
    if fs::write(&tmp, content).is_ok() {
        let _ = fs::rename(&tmp, work_file);
    }
}

// ── Subprocess-free repo introspection ──────────────────────────────────

/// Resolve `(sha, branch)` of HEAD by reading git's files directly.
/// Falls back to `git rev-parse` when file-reading fails (exotic layouts,
/// reftable backends, mid-update races).
pub fn head_state(project_root: &str) -> Option<(String, String)> {
    if let Some(result) = fast_head(Path::new(project_root)) {
        return Some(result);
    }
    let cfg = crate::config::Config::load_or_default();
    let sha =
        crate::git::proxy::run_git_capture_in(&cfg, &["rev-parse", "HEAD"], Some(project_root))
            .ok()?;
    let branch = crate::git::proxy::run_git_capture_in(
        &cfg,
        &["rev-parse", "--abbrev-ref", "HEAD"],
        Some(project_root),
    )
    .unwrap_or_else(|_| "HEAD".into());
    Some((sha, branch))
}

/// Per-worktree git dir: `.git` when it is a directory, or the `gitdir:`
/// target when `.git` is a worktree pointer file.
fn worktree_git_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    let meta = fs::symlink_metadata(&dot_git).ok()?;
    if meta.is_dir() {
        return Some(dot_git);
    }
    // `.git` file: "gitdir: /path/to/gitdir" (possibly relative).
    let content = fs::read_to_string(&dot_git).ok()?;
    let target = content.strip_prefix("gitdir:")?.trim();
    let path = Path::new(target);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    })
}

/// The git *common* dir (shared object/ref store across worktrees),
/// resolved without subprocesses.
fn fast_git_common_dir(root: &Path) -> Option<PathBuf> {
    let git_dir = worktree_git_dir(root)?;
    let commondir_file = git_dir.join("commondir");
    if let Ok(content) = fs::read_to_string(&commondir_file) {
        let target = Path::new(content.trim());
        let resolved = if target.is_absolute() {
            target.to_path_buf()
        } else {
            git_dir.join(target)
        };
        return Some(normalize(&resolved));
    }
    Some(git_dir)
}

/// Lexically resolve `..` segments (the `commondir` file commonly holds
/// relative paths like `../..`). No filesystem access, no symlink games.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Read HEAD → `(sha, branch)` from git's files.
fn fast_head(root: &Path) -> Option<(String, String)> {
    let git_dir = worktree_git_dir(root)?;
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();

    if let Some(refname) = head.strip_prefix("ref:") {
        let refname = refname.trim();
        let branch = refname
            .strip_prefix("refs/heads/")
            .unwrap_or(refname)
            .to_string();
        let common = fast_git_common_dir(root)?;
        let sha = resolve_ref(&common, refname)?;
        return Some((sha, branch));
    }

    // Detached HEAD: the file holds the sha directly.
    if head.len() >= 40 && head.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some((head.to_string(), "HEAD".to_string()));
    }
    None
}

/// Resolve a fully-qualified ref to a sha: loose ref file first, then
/// `packed-refs`.
fn resolve_ref(common_dir: &Path, refname: &str) -> Option<String> {
    if let Ok(content) = fs::read_to_string(common_dir.join(refname)) {
        let sha = content.trim();
        if !sha.is_empty() {
            return Some(sha.to_string());
        }
    }
    let packed = fs::read_to_string(common_dir.join("packed-refs")).ok()?;
    for line in packed.lines() {
        if line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        if let Some((sha, name)) = line.split_once(' ') {
            if name.trim() == refname {
                return Some(sha.trim().to_string());
            }
        }
    }
    None
}

// ── Worker singleton lock ───────────────────────────────────────────────

/// A worker considered dead after this long without a heartbeat.
const LOCK_STALE_SECS: u64 = 600;

pub struct WorkerLock {
    path: PathBuf,
}

impl WorkerLock {
    /// Try to become THE worker for this repo. Returns `None` when a
    /// fresh lock is held by someone else. Stale locks (crashed workers)
    /// are stolen.
    pub fn acquire(project_root: &str) -> Option<WorkerLock> {
        let dir = spool_dir(project_root)?;
        fs::create_dir_all(&dir).ok()?;
        let path = dir.join("worker.lock");

        if let Some(lock) = Self::try_create(&path) {
            return Some(lock);
        }

        let stale = fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age.as_secs() > LOCK_STALE_SECS);
        if stale {
            let _ = fs::remove_file(&path);
            // Single retry; if another process wins the race, defer.
            return Self::try_create(&path);
        }
        None
    }

    fn try_create(path: &Path) -> Option<WorkerLock> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .ok()?;
        let _ = writeln!(f, "{}", std::process::id());
        Some(WorkerLock {
            path: path.to_path_buf(),
        })
    }

    /// Refresh the lock mtime so long drains aren't stolen as stale.
    pub fn heartbeat(&self) {
        let _ = fs::write(&self.path, format!("{}\n", std::process::id()));
    }
}

impl Drop for WorkerLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo() -> Option<(tempfile::TempDir, String)> {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_str().unwrap().to_string();
        let ok = std::process::Command::new("git")
            .args(["init", &repo])
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
            let _ = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output();
        }
        let _ = std::process::Command::new("git")
            .args(["-C", &repo, "commit", "--allow-empty", "-m", "init"])
            .output();
        Some((tmp, repo))
    }

    #[test]
    fn fast_head_matches_git() {
        let Some((_tmp, repo)) = init_repo() else {
            return;
        };
        let (sha, branch) = fast_head(Path::new(&repo)).expect("fast head");
        let expected = std::process::Command::new("git")
            .args(["-C", &repo, "rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert_eq!(sha, String::from_utf8_lossy(&expected.stdout).trim());
        assert!(!branch.is_empty() && branch != "HEAD");
    }

    #[test]
    fn fast_head_resolves_packed_refs() {
        let Some((_tmp, repo)) = init_repo() else {
            return;
        };
        let _ = std::process::Command::new("git")
            .args(["-C", &repo, "pack-refs", "--all"])
            .output();
        // Loose ref is gone; resolution must go through packed-refs.
        let (sha, _) = fast_head(Path::new(&repo)).expect("packed ref resolution");
        assert_eq!(sha.len(), 40);
    }

    #[test]
    fn fast_head_in_linked_worktree() {
        let Some((_tmp, repo)) = init_repo() else {
            return;
        };
        let wt = format!("{repo}-wt");
        let ok = std::process::Command::new("git")
            .args(["-C", &repo, "worktree", "add", &wt, "-b", "side"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            return;
        }
        let (sha, branch) = fast_head(Path::new(&wt)).expect("worktree head");
        assert_eq!(sha.len(), 40);
        assert_eq!(branch, "side");

        // Worktree spool routes to the SHARED common dir. Canonicalize:
        // on macOS the worktree gitdir stores the /private/var form of
        // the same /var path — one physical directory either way.
        fs::create_dir_all(spool_dir(&repo).unwrap()).unwrap();
        let main_dir = fs::canonicalize(spool_dir(&repo).unwrap()).unwrap();
        let wt_dir = fs::canonicalize(spool_dir(&wt).unwrap()).unwrap();
        assert_eq!(main_dir, wt_dir, "worktrees share one spool");
        let _ = fs::remove_dir_all(&wt);
    }

    #[test]
    fn append_take_complete_roundtrip() {
        let Some((_tmp, repo)) = init_repo() else {
            return;
        };

        let e1 = append_commit(&repo).unwrap();
        assert!(has_pending(&repo));
        // Duplicate append (proxy + post-commit hook both fire).
        append_entry(&repo, &e1).unwrap();
        let e2 = SpoolEntry {
            root: repo.clone(),
            sha: "deadbeef".into(),
            branch: "main".into(),
            ts: 1,
        };
        append_entry(&repo, &e2).unwrap();

        let work = take_work(&repo);
        assert_eq!(work.len(), 1);
        assert!(!spool_file(&spool_dir(&repo).unwrap()).exists());

        let entries = read_entries(&work[0]);
        assert_eq!(entries.len(), 2, "duplicates collapse: {entries:?}");
        assert_eq!(entries[0], e1);
        assert_eq!(entries[1], e2);

        // Partial failure → failed entry survives for retry.
        complete_work(&work[0], std::slice::from_ref(&e2));
        assert!(work[0].exists());
        assert_eq!(read_entries(&work[0]), vec![e2]);

        // Full success → work file gone, nothing pending.
        complete_work(&work[0], &[]);
        assert!(!work[0].exists());
        assert!(!has_pending(&repo));
    }

    #[test]
    fn crashed_worker_leftovers_are_picked_up() {
        let Some((_tmp, repo)) = init_repo() else {
            return;
        };
        append_commit(&repo).unwrap();
        let first = take_work(&repo);
        assert_eq!(first.len(), 1);
        // "Crash": work file left behind. New commit lands meanwhile.
        append_commit(&repo).unwrap();
        let second = take_work(&repo);
        assert_eq!(second.len(), 2, "leftover + fresh work both claimed");
        for f in &second {
            complete_work(f, &[]);
        }
    }

    #[test]
    fn malformed_lines_are_dropped_not_fatal() {
        let Some((_tmp, repo)) = init_repo() else {
            return;
        };
        let dir = spool_dir(&repo).unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            spool_file(&dir),
            "not json\n{\"root\":\"/r\",\"sha\":\"abc\",\"branch\":\"b\",\"ts\":1}\n\n",
        )
        .unwrap();
        let work = take_work(&repo);
        let entries = read_entries(&work[0]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].sha, "abc");
        complete_work(&work[0], &[]);
    }

    #[test]
    fn worker_lock_is_exclusive_and_stealable_when_stale() {
        let Some((_tmp, repo)) = init_repo() else {
            return;
        };
        let lock = WorkerLock::acquire(&repo).expect("first acquire");
        assert!(
            WorkerLock::acquire(&repo).is_none(),
            "second acquire must fail while held"
        );
        drop(lock);
        let lock2 = WorkerLock::acquire(&repo).expect("reacquire after drop");
        drop(lock2);

        // Stale lock (old mtime) gets stolen.
        let path = spool_dir(&repo).unwrap().join("worker.lock");
        fs::write(&path, "99999\n").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let f = fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.set_times(fs::FileTimes::new().set_modified(old)).unwrap();
        drop(f);
        let stolen = WorkerLock::acquire(&repo);
        assert!(stolen.is_some(), "stale lock must be stealable");
    }
}
