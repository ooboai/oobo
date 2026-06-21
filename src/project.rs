//! Stable project identity.
//!
//! Projects are resolved in this order:
//!
//! 1. **Canonical git remote URL**  --  git@github.com:owner/repo.git and
//!    https://github.com/owner/repo become the same `owner/repo` key.
//! 2. **Filesystem path**  --  the repo's `project_root`. Path-based identity
//!    is fragile (project moved, renamed) but always available.
//! 3. **Initial commit SHA + basename**  --  for repos with no remote but at
//!    least one commit, stable across moves.
//!
//! This module is the single source of truth for `project_id` derivation.
//! Callers should use [`id_for_root`] rather than re-implementing the logic.

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

    // Strip scheme first so we can uniformly handle userinfo.
    for scheme in ["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = s.strip_prefix(scheme) {
            s = rest.to_string();
            break;
        }
    }

    // git@host:path → host/path (SSH shorthand)
    if let Some(at) = s.find('@') {
        let after_at = &s[at + 1..];
        if let Some(colon) = after_at.find(':') {
            // Only treat as SSH shorthand if the colon is NOT followed by //
            // and no slash comes before it (i.e. host:path, not host/path:port).
            let before_colon = &after_at[..colon];
            if !before_colon.contains('/') && !after_at[colon..].starts_with("://") {
                let host = before_colon;
                let path = &after_at[colon + 1..];
                s = format!("{host}/{path}");
            } else {
                // URL with userinfo (user:pass@host/path) — drop everything before @
                s = after_at.to_string();
            }
        } else {
            // user@host/path — drop userinfo
            s = after_at.to_string();
        }
    }

    // Drop residual user info (git@host/...) in case scheme-strip didn't cover it.
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
        _ => format!("p:{}", crate::paths::slug_from_path(path)),
    }
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
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Resolve the stable project id for `root` without requiring a Config.
/// Uses the git remote when available, falls back to a path-based id.
/// Safe to call from hooks and read-only code paths.
pub fn id_for_root(root: &str) -> String {
    let remote = detect_remote_for(root);
    derive_id(remote.as_deref(), root)
}

// ── Machine-local repo registry ─────────────────────────────────────────
//
// project_id → last-known root on THIS machine. Written opportunistically
// (worker drain, session-start), read by the pointer-resolution chain so
// a session homed in repo X resolves locally when X is checked out here —
// no network, no backend. Never synced; purely a local hint.

fn registry_path() -> std::path::PathBuf {
    crate::paths::oobo_home()
        .join("state")
        .join("repo-registry.json")
}

type Registry = std::collections::HashMap<String, RegistryEntry>;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct RegistryEntry {
    pub root: String,
    pub updated_at: i64,
}

fn load_registry() -> Registry {
    std::fs::read_to_string(registry_path())
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

/// Record (or refresh) the current root for this repo's project id.
/// Cheap no-op when the entry is already current.
pub fn registry_note(root: &str) {
    let canon = std::fs::canonicalize(root)
        .map_or_else(|_| root.to_string(), |p| p.to_string_lossy().to_string());
    let id = id_for_root(&canon);

    let mut reg = load_registry();
    if reg.get(&id).is_some_and(|e| e.root == canon) {
        return;
    }
    reg.insert(
        id,
        RegistryEntry {
            root: canon,
            updated_at: chrono::Utc::now().timestamp(),
        },
    );

    let path = registry_path();
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    if let Ok(json) = serde_json::to_string_pretty(&reg) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Last-known local root for a project id, verified to still be a git
/// repo. Stale entries (moved/deleted checkouts) resolve to `None`.
pub fn registry_lookup(project_id: &str) -> Option<String> {
    let reg = load_registry();
    let entry = reg.get(project_id)?;
    let git_marker = std::path::Path::new(&entry.root).join(".git");
    if git_marker.exists() {
        Some(entry.root.clone())
    } else {
        None
    }
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
    fn test_canonicalize_embedded_credentials() {
        // PAT-authenticated URL (GitHub Actions, bench servers, etc.)
        assert_eq!(
            canonicalize_remote("https://x-access-token:ghp_abc123@github.com/oobobench/hono.git"),
            "github.com/oobobench/hono"
        );
        // Basic auth style
        assert_eq!(
            canonicalize_remote("https://user:password@github.com/org/repo.git"),
            "github.com/org/repo"
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
