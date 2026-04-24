//! Stable project identity.
//!
//! Projects are resolved in this order:
//!
//! 1. **Canonical git remote URL** — git@github.com:owner/repo.git and
//!    https://github.com/owner/repo become the same `owner/repo` key.
//! 2. **Filesystem path** — the repo's `project_root`. Path-based identity
//!    is fragile (project moved, renamed) but always available.
//! 3. **Initial commit SHA + basename** — for repos with no remote but at
//!    least one commit, stable across moves.
//!
//! This module is the single source of truth for `project_id` derivation.
//! Callers should use [`derive_id`] and [`resolve_or_create`] rather than
//! re-implementing the logic.

use crate::config::Config;
use crate::db::projects::{path_to_project_id, ProjectRow};
use crate::db::Db;

/// Canonicalize a git remote URL so the SSH and HTTPS forms produce the
/// same key.
///
/// Examples (all → `github.com/acme/widget`):
/// - `git@github.com:acme/widget.git`
/// - `https://github.com/acme/widget`
/// - `https://github.com/acme/widget.git`
/// - `ssh://git@github.com/acme/widget.git`
pub fn canonicalize_remote(url: &str) -> String {
    let mut s = url.trim().to_string();
    s = s.trim_end_matches(".git").to_string();

    // git@host:path → host/path
    if let Some(at) = s.find('@') {
        if let Some(colon) = s[at..].find(':') {
            let host = &s[at + 1..at + colon];
            let path = &s[at + colon + 1..];
            s = format!("{host}/{path}");
        }
    }
    // Strip scheme.
    for scheme in ["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = s.strip_prefix(scheme) {
            s = rest.to_string();
            break;
        }
    }
    // Drop user info (git@host/...).
    if let Some(rest) = s.strip_prefix("git@") {
        s = rest.to_string();
    }
    s.trim_matches('/').to_lowercase()
}

/// Derive a project id from a canonicalized remote url or fall back to path.
///
/// Prefixes are used to keep remote-derived and path-derived IDs in
/// distinct namespaces:
/// - `r:github.com/acme/widget`
/// - `p:Users-me-dev-widget`
pub fn derive_id(remote: Option<&str>, path: &str) -> String {
    match remote {
        Some(r) if !r.is_empty() => format!("r:{}", canonicalize_remote(r)),
        _ => format!("p:{}", path_to_project_id(path)),
    }
}

/// Result of project resolution: existing (exact match), moved/cloned
/// (found by remote or initial commit but path differs), or new.
pub enum ProjectResolution {
    Existing(ProjectRow),
    MovedOrCloned {
        project: ProjectRow,
        new_path: String,
    },
    New {
        remote: Option<String>,
        initial_commit: Option<String>,
        path: String,
    },
}

/// Find a project row or create one if it doesn't exist.
///
/// Lookup order:
/// 1. By canonical remote (looking at the `git_remote` column).
/// 2. By initial commit SHA (for repos without a remote).
/// 3. By path.
/// 4. New row with stable id.
pub fn resolve_or_create(db: &Db, cfg: &Config, root: &str) -> Result<ProjectRow, String> {
    match resolve(db, cfg, root)? {
        ProjectResolution::Existing(p) => Ok(p),
        ProjectResolution::MovedOrCloned { mut project, new_path } => {
            let old_path = project.path.clone();
            project.path = new_path;
            project.last_seen_at = chrono::Utc::now().timestamp();
            if !project.historical_paths.contains(&old_path) {
                project.historical_paths.push(old_path);
            }
            db.upsert_project(&project)?;
            Ok(project)
        }
        ProjectResolution::New { remote, initial_commit, path } => {
            let id = derive_id(remote.as_deref(), &path);
            let name = std::path::Path::new(&path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let now = chrono::Utc::now().timestamp();
            let row = ProjectRow {
                id,
                path,
                name,
                git_remote: remote,
                initial_commit_sha: initial_commit,
                historical_paths: Vec::new(),
                discovered_at: now,
                last_seen_at: now,
                last_scanned_at: 0,
                tools: Vec::new(),
            };
            db.upsert_project(&row)?;
            Ok(row)
        }
    }
}

/// Pure resolution without side effects.
fn resolve(db: &Db, cfg: &Config, root: &str) -> Result<ProjectResolution, String> {
    let remote = detect_remote(cfg, root);
    let canonical = remote.as_deref().map(canonicalize_remote);
    let initial_commit = detect_initial_commit(root);

    if let Some(ref c) = canonical {
        if let Some(p) = find_by_canonical_remote(db, c)? {
            if p.path != root {
                return Ok(ProjectResolution::MovedOrCloned {
                    project: p,
                    new_path: root.to_string(),
                });
            }
            return Ok(ProjectResolution::Existing(p));
        }
    }
    if let Some(ref sha) = initial_commit {
        if let Some(p) = db.get_project_by_initial_commit(sha)? {
            if p.path != root {
                return Ok(ProjectResolution::MovedOrCloned {
                    project: p,
                    new_path: root.to_string(),
                });
            }
            return Ok(ProjectResolution::Existing(p));
        }
    }
    if let Some(p) = db.get_project_by_path(root)? {
        return Ok(ProjectResolution::Existing(p));
    }
    Ok(ProjectResolution::New {
        remote,
        initial_commit,
        path: root.to_string(),
    })
}

fn detect_remote(_cfg: &Config, root: &str) -> Option<String> {
    detect_remote_for(root)
}

fn detect_remote_for(root: &str) -> Option<String> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let output = std::process::Command::new(&git)
        .args(["remote", "get-url", "origin"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn detect_initial_commit(root: &str) -> Option<String> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let output = std::process::Command::new(&git)
        .args(["rev-list", "--max-parents=0", "HEAD"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout);
    s.lines().last().map(|l| l.trim().to_string()).filter(|l| !l.is_empty())
}

/// Resolve the stable project id for `root` without requiring a Config
/// or Db. Uses the git remote when available, falls back to a path-based
/// id. Safe to call in hooks, scanners, and read-only code paths.
pub fn id_for_root(root: &str) -> String {
    let remote = detect_remote_for(root);
    derive_id(remote.as_deref(), root)
}

/// Atomically upsert a `projects` row for `root` using the stable id and
/// its detected git remote, and return the id.
///
/// Handles the in-flight transition case where an old row for the same
/// `path` but a legacy id already exists (v9 migration did not have a
/// `git_remote` value to canonicalize at the time). That row is renamed
/// to the stable id and its children are re-parented.
pub fn ensure_stable(db: &Db, root: &str) -> Result<String, String> {
    let remote = detect_remote_for(root);
    let initial_commit = detect_initial_commit(root);
    let id = derive_id(remote.as_deref(), root);

    if let Some(existing) = db.get_project_by_path(root)? {
        if existing.id != id {
            rename_project(db, &existing.id, &id)?;
        }
    }

    let name = std::path::Path::new(root)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let now = chrono::Utc::now().timestamp();
    let row = ProjectRow {
        id: id.clone(),
        path: root.to_string(),
        name,
        git_remote: remote,
        initial_commit_sha: initial_commit,
        historical_paths: Vec::new(),
        discovered_at: now,
        last_seen_at: now,
        last_scanned_at: 0,
        tools: Vec::new(),
    };
    db.upsert_project(&row)?;
    Ok(id)
}

fn rename_project(db: &Db, from: &str, to: &str) -> Result<(), String> {
    // Mirror of v9's per-project rewrite. FKs stay ON, so we defer them
    // for the duration of the transaction — the DB only checks
    // referential integrity at commit time.
    let conn = &db.conn;
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("rename tx: {e}"))?;
    tx.execute_batch("PRAGMA defer_foreign_keys = ON;")
        .map_err(|e| format!("rename defer fk: {e}"))?;
    for table in [
        "ai_commits",
        "sessions",
        "events",
        "git_activity",
        "project_settings",
    ] {
        tx.execute(
            &format!("UPDATE {table} SET project_id = ?1 WHERE project_id = ?2"),
            rusqlite::params![to, from],
        )
        .map_err(|e| format!("rename {table}: {e}"))?;
    }
    tx.execute(
        "UPDATE projects SET id = ?1 WHERE id = ?2",
        rusqlite::params![to, from],
    )
    .map_err(|e| format!("rename projects: {e}"))?;
    tx.commit().map_err(|e| format!("rename commit: {e}"))?;
    Ok(())
}

fn find_by_canonical_remote(
    db: &Db,
    canonical: &str,
) -> Result<Option<ProjectRow>, String> {
    let sql = format!(
        "SELECT {} FROM projects WHERE git_remote IS NOT NULL",
        crate::db::Db::PROJECT_COLS
    );
    let mut stmt = db.conn.prepare(&sql).map_err(|e| format!("prepare canonical lookup: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            let hist_json: String = row.get(5).unwrap_or_else(|_| "[]".to_string());
            let historical_paths: Vec<String> = serde_json::from_str(&hist_json).unwrap_or_default();
            let tools_json: String = row.get(9).unwrap_or_default();
            let tools: Vec<String> = serde_json::from_str(&tools_json).unwrap_or_default();
            Ok(ProjectRow {
                id: row.get(0)?,
                path: row.get(1)?,
                name: row.get(2)?,
                git_remote: row.get(3)?,
                initial_commit_sha: row.get(4).unwrap_or_default(),
                historical_paths,
                discovered_at: row.get(6).unwrap_or(0),
                last_seen_at: row.get(7).unwrap_or(0),
                last_scanned_at: row.get(8).unwrap_or(0),
                tools,
            })
        })
        .map_err(|e| format!("query canonical: {e}"))?;
    for r in rows.flatten() {
        if let Some(ref gr) = r.git_remote {
            if canonicalize_remote(gr) == canonical {
                return Ok(Some(r));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonicalize_ssh_vs_https() {
        let a = canonicalize_remote("git@github.com:acme/widget.git");
        let b = canonicalize_remote("https://github.com/acme/widget");
        let c = canonicalize_remote("https://github.com/acme/widget.git");
        let d = canonicalize_remote("ssh://git@github.com/acme/widget.git");
        assert_eq!(a, "github.com/acme/widget");
        assert_eq!(b, "github.com/acme/widget");
        assert_eq!(c, "github.com/acme/widget");
        assert_eq!(d, "github.com/acme/widget");
    }

    #[test]
    fn test_canonicalize_case_insensitive() {
        assert_eq!(
            canonicalize_remote("https://GitHub.com/Acme/Widget"),
            "github.com/acme/widget"
        );
    }

    #[test]
    fn test_derive_id_remote() {
        let id = derive_id(Some("git@github.com:acme/widget.git"), "/anywhere");
        assert_eq!(id, "r:github.com/acme/widget");
    }

    #[test]
    fn test_derive_id_path_fallback() {
        let id = derive_id(None, "/Users/me/proj");
        assert_eq!(id, "p:Users-me-proj");
    }
}
