//! Per-commit provenance cache.
//!
//! Keyed by the commit sha — which makes rewrite invalidation automatic:
//! `rekey_anchors` re-points anchors at the new sha, the old cache entry
//! simply stops being addressed (a cache miss, not data loss), and the
//! next query recomputes from content. Entries are immutable once
//! written because the inputs (commit blobs + captured edits) are.

use std::path::{Path, PathBuf};

use super::FileProvenance;

fn entry_path(repo_root: &str, sha: &str, file_path: &str) -> PathBuf {
    // FNV-1a over the file path keeps names flat and filesystem-safe.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in file_path.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Path::new(repo_root)
        .join(".oobo")
        .join("cache")
        .join("provenance")
        .join(&sha[..2.min(sha.len())])
        .join(format!("{sha}-{h:016x}.json"))
}

pub fn read(repo_root: &str, sha: &str, file_path: &str) -> Option<FileProvenance> {
    let content = std::fs::read_to_string(entry_path(repo_root, sha, file_path)).ok()?;
    let cached: FileProvenance = serde_json::from_str(&content).ok()?;
    // Schema bump or path-hash collision → recompute.
    if cached.schema_version != super::PROVENANCE_SCHEMA_VERSION || cached.path != file_path {
        return None;
    }
    Some(cached)
}

pub fn write(repo_root: &str, provenance: &FileProvenance) {
    let path = entry_path(repo_root, &provenance.commit, &provenance.path);
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(json) = serde_json::to_string(provenance) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
    prune_stale(repo_root);
}

/// Opportunistic size control: at most once a day (marker-file mtime),
/// drop cache entries untouched for 30 days. Entries are pure derived
/// data — eviction is always safe, the next query recomputes.
fn prune_stale(repo_root: &str) {
    const PRUNE_INTERVAL: u64 = 24 * 60 * 60;
    const MAX_AGE: u64 = 30 * 24 * 60 * 60;

    let root = Path::new(repo_root)
        .join(".oobo")
        .join("cache")
        .join("provenance");
    let marker = root.join(".last-prune");
    let now = std::time::SystemTime::now();
    if let Ok(meta) = std::fs::metadata(&marker) {
        let fresh = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age.as_secs() < PRUNE_INTERVAL);
        if fresh {
            return;
        }
    }
    let _ = std::fs::write(&marker, b"");

    let Ok(shards) = std::fs::read_dir(&root) else {
        return;
    };
    for shard in shards.flatten().filter(|e| e.path().is_dir()) {
        let Ok(entries) = std::fs::read_dir(shard.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let stale = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|m| now.duration_since(m).ok())
                .is_some_and(|age| age.as_secs() > MAX_AGE);
            if stale {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::{LineOrigin, PROVENANCE_SCHEMA_VERSION};

    fn mk(commit: &str, path: &str) -> FileProvenance {
        FileProvenance {
            schema_version: PROVENANCE_SCHEMA_VERSION,
            commit: commit.into(),
            path: path.into(),
            baseline_blob: "b".into(),
            committed_blob: "c".into(),
            steps: Vec::new(),
            gaps: Vec::new(),
            clobbers: Vec::new(),
            lines: vec![LineOrigin::Baseline],
        }
    }

    #[test]
    fn roundtrip_and_keying() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();

        let p = mk("abc123def", "src/a.rs");
        write(root, &p);

        let back = read(root, "abc123def", "src/a.rs").expect("cache hit");
        assert_eq!(back.lines, p.lines);

        // Different path or sha → miss.
        assert!(read(root, "abc123def", "src/b.rs").is_none());
        assert!(read(root, "ffff00ff", "src/a.rs").is_none());
    }

    #[test]
    fn schema_bump_invalidates() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();

        let mut p = mk("abc123def", "src/a.rs");
        p.schema_version = 0; // stale schema on disk
        write(root, &p);
        assert!(read(root, "abc123def", "src/a.rs").is_none());
    }
}
