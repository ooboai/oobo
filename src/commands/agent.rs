use std::fs;
use std::path::PathBuf;

use crate::paths;

const SKILL_MD: &str = include_str!("../../skills/core/SKILL.md");

fn oobo_skill_dir() -> PathBuf {
    paths::oobo_home().join("skills").join("oobo")
}

fn oobo_skill_path() -> PathBuf {
    oobo_skill_dir().join("SKILL.md")
}

pub fn ensure_skill_file() -> Result<PathBuf, String> {
    let skill_dir = oobo_skill_dir();
    paths::ensure_dir(&skill_dir)?;

    let path = oobo_skill_path();
    let needs_write = if path.exists() {
        let existing = fs::read_to_string(&path).unwrap_or_default();
        existing != SKILL_MD
    } else {
        true
    };

    if needs_write {
        fs::write(&path, SKILL_MD).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }

    ensure_all_symlinks(&skill_dir);

    Ok(path)
}

fn ensure_all_symlinks(skill_dir: &PathBuf) {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return,
    };

    // .agents/skills is the universal convention — always create
    ensure_symlink(skill_dir, &home.join(".agents").join("skills").join("oobo"));

    // Tool-specific paths: only create if the tool's base directory already exists,
    // to avoid prematurely creating config directories for tools not in use.
    let conditional = [
        (".claude", "skills", "oobo"),
        (".codex", "skills", "oobo"),
        (".cursor", "skills", "oobo"),
        (".gemini", "skills", "oobo"),
    ];

    for (base, sub, name) in &conditional {
        let base_dir = home.join(base);
        if base_dir.exists() {
            ensure_symlink(skill_dir, &base_dir.join(sub).join(name));
        }
    }
}

fn ensure_symlink(skill_dir: &PathBuf, link: &PathBuf) {
    let link_parent = match link.parent() {
        Some(p) => p,
        None => return,
    };

    if paths::ensure_dir(link_parent).is_err() {
        return;
    }

    if link.exists() || link.symlink_metadata().is_ok() {
        if link.is_symlink() {
            if let Ok(target) = fs::read_link(link) {
                if target == *skill_dir {
                    return;
                }
            }
            let _ = fs::remove_file(link);
        } else {
            return;
        }
    }

    #[cfg(unix)]
    {
        let _ = std::os::unix::fs::symlink(skill_dir, link);
    }

    #[cfg(windows)]
    {
        let _ = std::os::windows::fs::symlink_dir(skill_dir, link);
    }
}
