//! `oobo sessions` / `oobo session …` — the session product surface.
//!
//! - `oobo sessions` lists every session this repo knows about: live
//!   capture state, locally-homed conversations, and foreign-home
//!   provenance stubs (with their pointer + hydration status).
//! - `oobo session show <uid>` follows the pointer chain and renders the
//!   resolved conversation (or the honest stub when access is missing).
//! - `oobo session share <uid> --to <repo>` deliberately re-homes a COPY
//!   of a conversation into another repo's v2 store.
//! - `oobo session migrate` re-points provenance stubs after the home
//!   remote configuration changed.

use crate::cli::OutputMode;
use crate::config::Config;
use crate::error::CmdResult;
use crate::git::orphan::v2;
use crate::git::orphan::v2::resolve::{resolve_conversation_with, Hydration};

/// One row in the listing.
#[derive(serde::Serialize)]
struct SessionRow {
    session_uid: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    native_session_ids: Vec<String>,
    tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    /// `None` = homed in this repo.
    #[serde(skip_serializing_if = "Option::is_none")]
    home_location: Option<String>,
    hydration: Hydration,
    /// Turns the session is KNOWN to have. Both the stored counter and
    /// the readable-conversation count are lower bounds of the truth
    /// (stubs have a counter but no readable conversation; legacy
    /// records have conversations but a zero counter), so this is the
    /// max of the two — same counter-max rule the store itself uses.
    turn_count: i64,
    /// Turns whose conversation payload is readable from this repo.
    /// 0 for a foreign stub without access — that's an access fact,
    /// not a claim about how many turns the session has.
    conversation_turns: usize,
    updated_at: i64,
    /// Still has live hook-capture state on this machine.
    live: bool,
}

pub fn run(cfg: &Config, resolve: bool, mode: OutputMode) -> CmdResult {
    let Some(root) = crate::git::proxy::project_root(cfg) else {
        eprintln!("oobo: not inside a git repository.");
        return Ok(1);
    };
    let repo_id = crate::project::id_for_root(&root);

    let live_sessions = crate::hooks::store::list_for_project(&root);
    let live_ids: std::collections::HashSet<String> = live_sessions
        .iter()
        .map(|s| crate::core::identity::session_uid(&s.agent, &s.session_id))
        .collect();

    let mut rows: Vec<SessionRow> = Vec::new();
    let mut stored: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (ts, uid) in v2::list_sessions_by_time(&root, &repo_id) {
        let Some(resolved) = resolve_conversation_with(&root, &repo_id, &uid, resolve) else {
            continue;
        };
        stored.insert(uid.clone());
        let stub = v2::read_provenance_session(&root, &repo_id, &uid);
        let home = stub.as_ref().and_then(|s| s.home_location.clone());
        let s = resolved.session;
        let stub_count = stub.as_ref().map_or(0, |st| st.turn_count);
        let turn_count = s
            .turn_count
            .max(stub_count)
            .max(resolved.conversation_turns as i64);
        rows.push(SessionRow {
            session_uid: uid.clone(),
            native_session_ids: s.native_session_ids,
            tool: s.tool,
            title: s.title,
            home_location: home,
            hydration: resolved.hydration,
            turn_count,
            conversation_turns: resolved.conversation_turns,
            updated_at: if s.updated_at > 0 { s.updated_at } else { ts },
            live: live_ids.contains(&uid),
        });
    }

    // Live sessions with no durable v2 record yet (in progress, nothing
    // committed/drained): still part of "every session this repo knows".
    for s in &live_sessions {
        let uid = crate::core::identity::session_uid(&s.agent, &s.session_id);
        if stored.contains(&uid) {
            continue;
        }
        rows.push(SessionRow {
            session_uid: uid,
            native_session_ids: vec![s.session_id.clone()],
            tool: crate::core::tool::normalize_source(&s.agent).to_string(),
            title: None,
            home_location: None,
            hydration: Hydration::Live,
            turn_count: s.current_turn_index,
            conversation_turns: 0,
            updated_at: s.updated_at,
            live: true,
        });
    }
    rows.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    if mode == OutputMode::Json {
        crate::utils::print_json(&serde_json::json!({
            "repo_id": repo_id,
            "sessions": rows,
        }));
    } else {
        {
            if rows.is_empty() {
                println!("no sessions recorded for this repo yet.");
                return Ok(0);
            }
            for r in &rows {
                let uid8 = &r.session_uid[..8.min(r.session_uid.len())];
                let home = match (&r.home_location, &r.hydration) {
                    (None, _) => "home".to_string(),
                    (Some(h), Hydration::StubOnly) => format!("@{h} (stub — no access)"),
                    (Some(h), Hydration::Cached { as_of }) => {
                        format!("@{h} (cached as of {})", fmt_ts(*as_of))
                    }
                    (Some(h), _) => format!("@{h}"),
                };
                let live = if r.live { " [live]" } else { "" };
                let title = r.title.as_deref().unwrap_or("(untitled)");
                println!(
                    "{uid8}  {:<9} {:>3}t  {title}  {home}{live}",
                    r.tool, r.turn_count
                );
            }
        }
    }
    Ok(0)
}

/// `oobo session show <uid>` — full pointer resolution for one session.
pub fn run_show(cfg: &Config, uid_arg: &str, mode: OutputMode) -> CmdResult {
    let Some(root) = crate::git::proxy::project_root(cfg) else {
        eprintln!("oobo: not inside a git repository.");
        return Ok(1);
    };
    let repo_id = crate::project::id_for_root(&root);
    let Some(uid) = resolve_uid(&root, &repo_id, uid_arg) else {
        eprintln!("oobo: no session matching '{uid_arg}' in this repo.");
        return Ok(1);
    };

    let Some(resolved) = resolve_conversation_with(&root, &repo_id, &uid, true) else {
        eprintln!("oobo: session '{uid}' not found.");
        return Ok(1);
    };
    let stub_count = v2::read_provenance_session(&root, &repo_id, &uid).map_or(0, |s| s.turn_count);
    let turn_count = resolved
        .session
        .turn_count
        .max(stub_count)
        .max(resolved.conversation_turns as i64);

    if mode == OutputMode::Json {
        crate::utils::print_json(&serde_json::json!({
            "session_uid": uid,
            "hydration": resolved.hydration,
            "turn_count": turn_count,
            "conversation_turns": resolved.conversation_turns,
            "session": resolved.session,
        }));
    } else {
        {
            let s = &resolved.session;
            println!("session {uid}");
            println!(
                "  tool: {}  model: {}",
                s.tool,
                s.model.as_deref().unwrap_or("-")
            );
            if let Some(t) = &s.title {
                println!("  title: {t}");
            }
            match &resolved.hydration {
                Hydration::Live => println!("  conversation: live capture (not stored yet)"),
                Hydration::Local => println!("  conversation: here (home store)"),
                Hydration::LocalRepo { root } => {
                    println!("  conversation: local checkout {root}");
                }
                Hydration::Fetched => println!("  conversation: fetched from home remote"),
                Hydration::Cached { as_of } => {
                    println!("  conversation: cached copy as of {}", fmt_ts(*as_of));
                }
                Hydration::StubOnly => println!(
                    "  conversation: not accessible from here (stub only{})",
                    s.home_location
                        .as_deref()
                        .map(|h| format!(" — home {h}"))
                        .unwrap_or_default()
                ),
            }
            println!(
                "  turns: {turn_count} ({} with conversation readable here)",
                resolved.conversation_turns
            );
            if !s.repos_touched.is_empty() {
                println!("  repos touched: {}", s.repos_touched.join(", "));
            }
        }
    }
    Ok(0)
}

/// `oobo session share <uid> --to <repo>` — consent-based copy of the
/// conversation into another repo's v2 store. The original stays put;
/// the copy is self-contained (home = the target).
pub fn run_share(cfg: &Config, uid_arg: &str, target: &str, mode: OutputMode) -> CmdResult {
    let Some(root) = crate::git::proxy::project_root(cfg) else {
        eprintln!("oobo: not inside a git repository.");
        return Ok(1);
    };
    let repo_id = crate::project::id_for_root(&root);
    let Some(uid) = resolve_uid(&root, &repo_id, uid_arg) else {
        eprintln!("oobo: no session matching '{uid_arg}' in this repo.");
        return Ok(1);
    };

    // The conversation must be readable from here (home or local checkout).
    let Some(resolved) = resolve_conversation_with(&root, &repo_id, &uid, true) else {
        eprintln!("oobo: session '{uid}' not found.");
        return Ok(1);
    };
    let source_root = match &resolved.hydration {
        Hydration::Local => root.clone(),
        Hydration::LocalRepo { root } => root.clone(),
        _ => {
            eprintln!(
                "oobo: conversation for '{uid}' is not locally readable — \
                 cannot share what we don't have."
            );
            return Ok(1);
        }
    };

    let Ok(target_root) = std::fs::canonicalize(target) else {
        eprintln!("oobo: target repo '{target}' not found.");
        return Ok(1);
    };
    let target_root = target_root.to_string_lossy().to_string();
    if crate::git::proxy::project_root_from(&target_root).is_empty() {
        eprintln!("oobo: target '{target}' is not a git repository.");
        return Ok(1);
    }

    // Copy: session record (home = target now) + every conversation turn.
    let mut record = resolved.session.clone();
    record.home_location = None; // self-contained in the target store
    if let Err(e) = v2::write_conversation_session(&target_root, &record) {
        eprintln!("oobo: share failed writing session record: {e}");
        return Ok(1);
    }
    let mut copied = 0usize;
    for idx in v2::list_conversation_turn_indices(&source_root, &uid) {
        if let Some((transcript, tool_calls)) = v2::read_conversation_turn(&source_root, &uid, idx)
        {
            match v2::write_conversation_turn(&target_root, &uid, idx, &transcript, &tool_calls) {
                Ok(true) => copied += 1,
                Ok(false) => {} // already present — idempotent share
                Err(e) => eprintln!("oobo: warning: turn {idx} not copied: {e}"),
            }
        }
    }

    match mode {
        OutputMode::Json => crate::utils::print_json(&serde_json::json!({
            "action": "share",
            "session_uid": uid,
            "target": target_root,
            "turns_copied": copied,
        })),
        _ => println!("shared session {uid} into {target_root} ({copied} turns copied)."),
    }
    Ok(0)
}

/// `oobo session migrate` — the home remote changed (`.oobo/config`
/// edited): re-point every provenance stub for sessions homed here.
pub fn run_migrate(cfg: &Config, mode: OutputMode) -> CmdResult {
    let Some(root) = crate::git::proxy::project_root(cfg) else {
        eprintln!("oobo: not inside a git repository.");
        return Ok(1);
    };
    let repo_id = crate::project::id_for_root(&root);
    let current_home = v2::home_location_for(&root);
    let self_id = crate::project::id_for_root(&root);

    let mut migrated = 0usize;
    for (_, uid) in v2::list_sessions_by_time(&root, &repo_id) {
        let Some(mut stub) = v2::read_provenance_session(&root, &repo_id, &uid) else {
            continue;
        };
        // Only sessions homed HERE migrate: their pointer (as seen from
        // other repos) is derived from this repo's config. A stub whose
        // home is elsewhere belongs to that home's migration.
        let homed_here = stub.home_location.is_none()
            || stub.home_location.as_deref() == Some(current_home.as_str())
            || stub.home_location.as_deref() == Some(self_id.as_str());
        if !homed_here {
            continue;
        }
        let desired = if current_home == self_id {
            None
        } else {
            Some(current_home.clone())
        };
        if stub.home_location == desired {
            continue;
        }
        stub.home_location = desired;
        stub.updated_at = chrono::Utc::now().timestamp();
        if v2::write_provenance_session(&root, &repo_id, &stub).is_ok() {
            migrated += 1;
        }
    }

    match mode {
        OutputMode::Json => crate::utils::print_json(&serde_json::json!({
            "action": "migrate",
            "home_location": current_home,
            "stubs_updated": migrated,
        })),
        _ => println!("home is {current_home}; {migrated} session pointer(s) updated."),
    }
    Ok(0)
}

/// Match a uid argument against stored sessions: exact uid, uid prefix,
/// or exact native session id.
fn resolve_uid(root: &str, repo_id: &str, arg: &str) -> Option<String> {
    let known = v2::list_sessions_by_time(root, repo_id);
    if known.iter().any(|(_, uid)| uid == arg) {
        return Some(arg.to_string());
    }
    let prefix: Vec<&String> = known
        .iter()
        .map(|(_, uid)| uid)
        .filter(|uid| uid.starts_with(arg))
        .collect();
    if prefix.len() == 1 {
        return Some(prefix[0].clone());
    }
    // Exact native id match via stubs.
    for (_, uid) in &known {
        if let Some(stub) = v2::read_provenance_session(root, repo_id, uid) {
            if stub.native_session_ids.iter().any(|n| n == arg) {
                return Some(uid.clone());
            }
        }
    }
    None
}

fn fmt_ts(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0).map_or_else(
        || ts.to_string(),
        |dt| dt.format("%Y-%m-%d %H:%M UTC").to_string(),
    )
}
