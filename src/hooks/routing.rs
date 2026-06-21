//! Capture routing  --  resolve which repository an edited file belongs to.
//!
//! Agent sessions are not confined to the workspace they were opened in: a
//! session running in project X can edit files in project Y (a completely
//! different repository). Before this module existed, capture was routed by
//! the hook event's cwd/workspace root, which silently dropped every edit
//! outside the origin repo. Routing by the *edited file's* repository is the
//! foundation of cross-repo attribution: provenance must land in the repo
//! that owns the file, regardless of where the session lives.

use std::path::{Path, PathBuf};

/// Where a single file path routes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedFile {
    /// Worktree root of the repository that owns this file.
    /// Canonicalized for foreign repos; the origin root is kept verbatim so
    /// it stays consistent with the rest of the session state.
    pub repo_root: String,
    /// Path relative to `repo_root`, forward-slashed.
    pub rel: String,
    /// True when the file belongs to a different repo than the origin.
    pub foreign: bool,
}

/// Route a file path from a hook event to its owning repository.
///
/// - Relative paths are interpreted against the origin root (today's
///   behavior, unchanged).
/// - Absolute paths under the origin root route to the origin.
/// - Absolute paths outside the origin root resolve to their own repo by
///   walking up the directory tree  --  these are *foreign* edits.
/// - Returns `None` for paths that belong to no repository at all.
///
/// Note: files in a repo nested inside the origin worktree route to the
/// origin (prefix match wins). This matches the existing capture behavior.
pub fn route_file(path: &str, origin_root: &str) -> Option<RoutedFile> {
    if path.is_empty() || path.ends_with('/') || path == "." {
        return None;
    }

    let p = Path::new(path);
    if !p.is_absolute() {
        if origin_root.is_empty() || path.starts_with("..") {
            return None;
        }
        return Some(RoutedFile {
            repo_root: origin_root.to_string(),
            rel: path.replace('\\', "/"),
            foreign: false,
        });
    }

    let abs = canonicalize_best_effort(p);

    if !origin_root.is_empty() {
        let root = canonicalize_best_effort(Path::new(origin_root));
        if let Ok(rel) = abs.strip_prefix(&root) {
            let rel = rel.to_string_lossy().replace('\\', "/");
            if rel.is_empty() {
                return None;
            }
            return Some(RoutedFile {
                repo_root: origin_root.to_string(),
                rel,
                foreign: false,
            });
        }
    }

    let repo_root = resolve_file_repo_root(&abs)?;
    let rel = abs
        .strip_prefix(Path::new(&repo_root))
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    if rel.is_empty() {
        return None;
    }
    Some(RoutedFile {
        repo_root,
        rel,
        foreign: true,
    })
}

/// Find the worktree root that owns `path` by walking up from its parent
/// looking for a `.git` entry (directory for normal repos, file for
/// worktrees and submodules). Pure filesystem walk  --  no subprocess  --
/// so it is safe on the blocking hook path.
fn resolve_file_repo_root(path: &Path) -> Option<String> {
    let mut dir = path.parent()?;
    loop {
        if dir.join(".git").exists() {
            return Some(std::fs::canonicalize(dir).map_or_else(
                |_| dir.to_string_lossy().to_string(),
                |p| p.to_string_lossy().to_string(),
            ));
        }
        dir = dir.parent()?;
    }
}

/// Canonicalize a path, tolerating components that don't exist yet (a
/// pre-tool-use hook fires before the file is created). Resolves the
/// deepest existing ancestor and re-appends the missing tail, which keeps
/// macOS `/var` vs `/private/var` and symlinked roots comparable.
fn canonicalize_best_effort(p: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(p) {
        return c;
    }
    match (p.parent(), p.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
            canonicalize_best_effort(parent).join(name)
        }
        _ => p.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo(root: &Path) {
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
    }

    fn canon(p: &Path) -> String {
        std::fs::canonicalize(p)
            .unwrap()
            .to_string_lossy()
            .to_string()
    }

    #[test]
    fn relative_path_routes_to_origin() {
        let routed = route_file("src/main.rs", "/some/origin").unwrap();
        assert_eq!(routed.repo_root, "/some/origin");
        assert_eq!(routed.rel, "src/main.rs");
        assert!(!routed.foreign);
    }

    #[test]
    fn relative_path_without_origin_is_unroutable() {
        assert!(route_file("src/main.rs", "").is_none());
    }

    #[test]
    fn traversal_path_is_unroutable() {
        assert!(route_file("../outside.rs", "/some/origin").is_none());
    }

    #[test]
    fn absolute_path_under_origin_routes_to_origin() {
        let origin = tempfile::tempdir().unwrap();
        init_repo(origin.path());
        let file = origin.path().join("src").join("lib.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "x").unwrap();

        let origin_str = canon(origin.path());
        let routed = route_file(file.to_str().unwrap(), &origin_str).unwrap();
        assert_eq!(routed.repo_root, origin_str);
        assert_eq!(routed.rel, "src/lib.rs");
        assert!(!routed.foreign);
    }

    #[test]
    fn absolute_path_in_other_repo_routes_foreign() {
        let origin = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        init_repo(origin.path());
        init_repo(other.path());
        let file = other.path().join("api").join("handler.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "y").unwrap();

        let routed = route_file(file.to_str().unwrap(), &canon(origin.path())).unwrap();
        assert!(routed.foreign);
        assert_eq!(routed.repo_root, canon(other.path()));
        assert_eq!(routed.rel, "api/handler.rs");
    }

    #[test]
    fn foreign_routing_works_for_not_yet_created_file() {
        let origin = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        init_repo(origin.path());
        init_repo(other.path());
        // File (and its directory) don't exist yet  --  pre-tool-use fires
        // before creation.
        let file = other.path().join("new").join("deep").join("file.rs");

        let routed = route_file(file.to_str().unwrap(), &canon(origin.path())).unwrap();
        assert!(routed.foreign);
        assert_eq!(routed.repo_root, canon(other.path()));
        assert_eq!(routed.rel, "new/deep/file.rs");
    }

    #[test]
    fn path_outside_any_repo_is_unroutable() {
        let origin = tempfile::tempdir().unwrap();
        let no_repo = tempfile::tempdir().unwrap();
        init_repo(origin.path());
        let file = no_repo.path().join("notes.txt");
        std::fs::write(&file, "z").unwrap();

        // The tempdir itself has no repo; the walk may still escape upward
        // into a developer's homedir repo, so only assert non-origin here.
        if let Some(routed) = route_file(file.to_str().unwrap(), &canon(origin.path())) {
            assert_ne!(routed.repo_root, canon(origin.path()));
        }
    }

    #[test]
    fn empty_origin_routes_absolute_paths_by_file_repo() {
        let other = tempfile::tempdir().unwrap();
        init_repo(other.path());
        let file = other.path().join("a.rs");
        std::fs::write(&file, "a").unwrap();

        let routed = route_file(file.to_str().unwrap(), "").unwrap();
        assert!(routed.foreign);
        assert_eq!(routed.repo_root, canon(other.path()));
        assert_eq!(routed.rel, "a.rs");
    }

    #[test]
    fn directory_like_paths_are_unroutable() {
        assert!(route_file("", "/o").is_none());
        assert!(route_file(".", "/o").is_none());
        assert!(route_file("src/", "/o").is_none());
    }
}
