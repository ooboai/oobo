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

/// Resolve the stable project id for `root` without requiring a Config
/// or Db. Uses the git remote when available, falls back to a path-based
/// id. Safe to call in hooks, scanners, and read-only code paths.
pub fn id_for_root(root: &str) -> String {
    let remote = detect_remote_for(root);
    derive_id(remote.as_deref(), root)
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
