//! Pointer resolution — turning a session stub into its conversation.
//!
//! A repo's provenance layer stores *stubs* (identity + `home_location`
//! pointer); the conversation lives exactly once, in the session's home
//! store. This module walks the resolution chain:
//!
//! 1. **This repo's conversation layer** — the session is homed here.
//! 2. **Machine-local registry** — the home repo is checked out on this
//!    machine; read its `anchors/v2` branch directly. Offline, instant.
//! 3. **Remote fetch** — the pointer names a remote (`r:host/owner/repo`);
//!    find a fetchable URL among the current repo's remotes / configured
//!    anchor remote, fetch the v2 branch into a cache repo, read from it.
//!    Access control is git's: no credentials → no conversation (stub).
//! 4. **Conversation cache** — a previously resolved copy in `~/.oobo`
//!    (read-only, served with an "as of" staleness marker when offline).
//! 5. **Stub only** — full attribution still works; the conversation
//!    needs access the user doesn't have from here.

use super::{SessionRecord, BRANCH};
use crate::git::orphan::git_in;

/// How the conversation was obtained.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Hydration {
    /// Live capture state only — nothing durably stored yet (session in
    /// progress, no commit drained). Listing-only state.
    Live,
    /// Conversation layer of the current repo (session homed here).
    Local,
    /// Read directly from another local checkout of the home repo.
    LocalRepo { root: String },
    /// Fetched from the home remote just now (and cached).
    Fetched,
    /// Served from the `~/.oobo` conversation cache.
    Cached { as_of: i64 },
    /// Pointer could not be followed — provenance stub only.
    StubOnly,
}

#[derive(Debug, Clone)]
pub struct ResolvedConversation {
    pub session: SessionRecord,
    pub hydration: Hydration,
    /// Conversation turns available at the resolved location.
    pub conversation_turns: usize,
}

/// Follow the pointer chain for one session referenced by this repo.
pub fn resolve_conversation(
    project_root: &str,
    repo_id: &str,
    session_uid: &str,
) -> Option<ResolvedConversation> {
    resolve_conversation_with(project_root, repo_id, session_uid, true)
}

/// Like [`resolve_conversation`] but with the network step optional —
/// listings resolve passively (local + cache only) so they stay instant.
pub fn resolve_conversation_with(
    project_root: &str,
    repo_id: &str,
    session_uid: &str,
    allow_fetch: bool,
) -> Option<ResolvedConversation> {
    // 1. Homed here.
    if let Some(session) = super::read_conversation_session(project_root, session_uid) {
        let turns = super::list_conversation_turn_indices(project_root, session_uid).len();
        return Some(ResolvedConversation {
            session,
            hydration: Hydration::Local,
            conversation_turns: turns,
        });
    }

    let stub = super::read_provenance_session(project_root, repo_id, session_uid)?;
    let Some(home) = stub.home_location.clone() else {
        // Home is this repo but the conversation layer is absent
        // (e.g. partial clone of the branch) — honest stub.
        return Some(stub_only(stub));
    };

    // 2. Home repo checked out locally.
    if let Some(root) = crate::project::registry_lookup(&home) {
        if let Some(session) = super::read_conversation_session(&root, session_uid) {
            let turns = super::list_conversation_turn_indices(&root, session_uid).len();
            return Some(ResolvedConversation {
                session,
                hydration: Hydration::LocalRepo { root },
                conversation_turns: turns,
            });
        }
    }

    // 3. Fetch via a URL that canonicalizes to the pointer.
    if allow_fetch {
        if let Some(url) = fetch_url_for(project_root, &home) {
            if let Some(resolved) = fetch_and_read(&url, session_uid) {
                cache_store(session_uid, &resolved);
                return Some(resolved);
            }
        }
    }

    // 4. Previously resolved copy, shown with its staleness marker.
    if let Some(cached) = cache_load(session_uid) {
        return Some(cached);
    }

    // 5. No access from here.
    Some(stub_only(stub))
}

fn stub_only(stub: SessionRecord) -> ResolvedConversation {
    ResolvedConversation {
        session: stub,
        hydration: Hydration::StubOnly,
        conversation_turns: 0,
    }
}

// ── Fetch-URL discovery ─────────────────────────────────────────────────

/// Find a fetchable URL whose canonical form matches the home pointer
/// (`r:host/owner/repo`). Sources, in order: the current repo's
/// configured anchor remote, then every git remote of the current repo.
fn fetch_url_for(project_root: &str, home_location: &str) -> Option<String> {
    let want = home_location.strip_prefix("r:")?;

    let mut candidates: Vec<String> = Vec::new();
    let global = crate::config::Config::load_or_default();
    let anchor_remote = crate::commands::sync::resolve(&global, Some(project_root)).anchor_remote;
    if !anchor_remote.is_empty() {
        // Remote *names* resolve to their URL; URLs and paths pass
        // through (same discrimination as `home_location_for`).
        if anchor_remote.contains("://")
            || anchor_remote.contains('@')
            || anchor_remote.contains('/')
        {
            candidates.push(anchor_remote);
        } else if let Ok(url) = git_in(project_root, &["remote", "get-url", &anchor_remote]) {
            candidates.push(url);
        }
    }
    if let Ok(remotes) = git_in(project_root, &["remote"]) {
        for name in remotes.lines() {
            if let Ok(url) = git_in(project_root, &["remote", "get-url", name.trim()]) {
                candidates.push(url);
            }
        }
    }

    candidates
        .into_iter()
        .find(|url| crate::project::canonicalize_remote(url) == want)
}

// ── Remote fetch into a cache repo ──────────────────────────────────────

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn cache_repo_dir(url: &str) -> std::path::PathBuf {
    crate::paths::oobo_home()
        .join("cache")
        .join("conversations")
        .join(format!("remote-{:016x}.git", fnv(url)))
}

/// Fetch the home store's v2 branch into a local cache repo and read the
/// session + its conversation-turn count from the fetched tip.
fn fetch_and_read(url: &str, session_uid: &str) -> Option<ResolvedConversation> {
    let dir = cache_repo_dir(url);
    std::fs::create_dir_all(&dir).ok()?;
    let dir_s = dir.to_str()?;

    if !dir.join("HEAD").exists() {
        let out = git_bare(dir_s, &["init", "--bare", "--quiet", dir_s])?;
        let _ = out;
    }
    git_bare(
        dir_s,
        &[
            "fetch",
            "--quiet",
            "--depth",
            "1",
            url,
            &format!("+refs/heads/{BRANCH}:refs/oobo/v2"),
        ],
    )?;

    let (prefix, rest) = crate::git::orphan::shard_key(session_uid);
    let base = format!("sessions/{prefix}/{rest}");
    let raw = git_bare(
        dir_s,
        &[
            "cat-file",
            "-p",
            &format!("refs/oobo/v2:{base}/session.json"),
        ],
    )?;
    let session: SessionRecord = serde_json::from_str(&raw).ok()?;

    let turns = git_bare(
        dir_s,
        &[
            "ls-tree",
            "--name-only",
            "refs/oobo/v2",
            &format!("{base}/turns/"),
        ],
    )
    .map_or(0, |listing| {
        listing.lines().filter(|l| !l.trim().is_empty()).count()
    });

    Some(ResolvedConversation {
        session,
        hydration: Hydration::Fetched,
        conversation_turns: turns,
    })
}

fn git_bare(git_dir: &str, args: &[&str]) -> Option<String> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let output = std::process::Command::new(git)
        .arg("--git-dir")
        .arg(git_dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

// ── Conversation cache (content-addressed, read-only) ──────────────────

#[derive(serde::Serialize, serde::Deserialize)]
struct CacheEntry {
    as_of: i64,
    conversation_turns: usize,
    session: SessionRecord,
}

fn cache_entry_path(session_uid: &str) -> std::path::PathBuf {
    crate::paths::oobo_home()
        .join("cache")
        .join("conversations")
        .join("sessions")
        .join(format!("{session_uid}.json"))
}

fn cache_store(session_uid: &str, resolved: &ResolvedConversation) {
    let path = cache_entry_path(session_uid);
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let entry = CacheEntry {
        as_of: chrono::Utc::now().timestamp(),
        conversation_turns: resolved.conversation_turns,
        session: resolved.session.clone(),
    };
    if let Ok(json) = serde_json::to_string(&entry) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

fn cache_load(session_uid: &str) -> Option<ResolvedConversation> {
    let raw = std::fs::read_to_string(cache_entry_path(session_uid)).ok()?;
    let entry: CacheEntry = serde_json::from_str(&raw).ok()?;
    Some(ResolvedConversation {
        session: entry.session,
        hydration: Hydration::Cached { as_of: entry.as_of },
        conversation_turns: entry.conversation_turns,
    })
}
