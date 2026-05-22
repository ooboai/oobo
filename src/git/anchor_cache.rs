//! Local JSON cache for orphan branch anchor data.
//!
//! Stores a snapshot of all anchors + session links at `.oobo/cache/anchors.json`.
//! The cache is keyed by the orphan branch tip commit — when the branch advances,
//! the cache is invalidated and rebuilt.

use crate::core::anchor::{Anchor, SessionLink};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(serde::Serialize, serde::Deserialize)]
struct CacheFile {
    branch_tip: String,
    anchors: Vec<Anchor>,
    session_links: HashMap<String, Vec<SessionLink>>,
}

fn cache_path(project_root: &str) -> PathBuf {
    Path::new(project_root)
        .join(".oobo")
        .join("cache")
        .join("anchors.json")
}

/// Load anchors from cache if valid, otherwise rebuild from orphan branch.
pub fn load_anchors_cached(project_root: &str) -> (Vec<Anchor>, HashMap<String, Vec<SessionLink>>) {
    let tip = super::orphan::branch_tip(project_root);

    if let Some(ref tip_hash) = tip {
        if let Some(cached) = read_cache(project_root, tip_hash) {
            return (cached.anchors, cached.session_links);
        }
    }

    let (anchors, links) = super::orphan::read_all_anchors(project_root);

    if let Some(tip_hash) = tip {
        let _ = write_cache(project_root, &tip_hash, &anchors, &links);
    }

    (anchors, links)
}

/// Invalidate the cache (e.g. after writing a new anchor).
pub fn invalidate(project_root: &str) {
    let path = cache_path(project_root);
    let _ = std::fs::remove_file(path);
}

fn read_cache(project_root: &str, expected_tip: &str) -> Option<CacheFile> {
    let path = cache_path(project_root);
    let content = std::fs::read_to_string(path).ok()?;
    let cache: CacheFile = serde_json::from_str(&content).ok()?;
    if cache.branch_tip == expected_tip {
        Some(cache)
    } else {
        None
    }
}

fn write_cache(
    project_root: &str,
    tip: &str,
    anchors: &[Anchor],
    links: &HashMap<String, Vec<SessionLink>>,
) -> Result<(), String> {
    let path = cache_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create cache dir: {e}"))?;
    }
    let cache = CacheFile {
        branch_tip: tip.to_string(),
        anchors: anchors.to_vec(),
        session_links: links.clone(),
    };
    let json = serde_json::to_string(&cache).map_err(|e| format!("serialize cache: {e}"))?;
    std::fs::write(path, json).map_err(|e| format!("write cache: {e}"))?;
    Ok(())
}
