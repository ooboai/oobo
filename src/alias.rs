use std::fs;
use std::path::PathBuf;

use crate::cli::AliasAction;

const ALIAS_MARKER: &str = "# oobo-git alias";
const ALIAS_LINE: &str = "alias git=oobo # oobo-git alias";

pub fn run(action: AliasAction) -> Result<(), String> {
    match action {
        AliasAction::Install => {
            install_alias()?;
            println!("Alias installed. Restart your shell or run:");
            for rc in detect_rc_files() {
                println!("  source {}", rc.display());
            }
            Ok(())
        }
        AliasAction::Uninstall => {
            uninstall_alias()?;
            println!("Alias removed. Restart your shell or run:");
            for rc in detect_rc_files() {
                println!("  source {}", rc.display());
            }
            Ok(())
        }
    }
}

/// Install the git→oobo alias in shell RC files.
pub fn install_alias() -> Result<(), String> {
    let rc_files = detect_rc_files();
    if rc_files.is_empty() {
        return Err("could not detect shell RC file".into());
    }

    for rc in &rc_files {
        let content = fs::read_to_string(rc).unwrap_or_default();
        if content.contains(ALIAS_MARKER) {
            continue; // already installed
        }
        let addition = format!("\n{ALIAS_LINE}\n");
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(rc)
            .and_then(|mut f| {
                use std::io::Write;
                f.write_all(addition.as_bytes())
            })
            .map_err(|e| format!("cannot write {}: {e}", rc.display()))?;
    }

    Ok(())
}

/// Remove the git→oobo alias from shell RC files.
pub fn uninstall_alias() -> Result<(), String> {
    let rc_files = detect_rc_files();

    for rc in &rc_files {
        let content = match fs::read_to_string(rc) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if !content.contains(ALIAS_MARKER) {
            continue;
        }

        let filtered: Vec<&str> = content
            .lines()
            .filter(|line| !line.contains(ALIAS_MARKER))
            .collect();

        fs::write(rc, filtered.join("\n") + "\n")
            .map_err(|e| format!("cannot write {}: {e}", rc.display()))?;
    }

    Ok(())
}

/// Detect which shell RC files exist.
fn detect_rc_files() -> Vec<PathBuf> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };

    let candidates = [
        (".zshrc", "zsh"),
        (".bashrc", "bash"),
        (".bash_profile", "bash"),
    ];

    let shell = std::env::var("SHELL").unwrap_or_default();
    let mut files = Vec::new();

    for (rc_name, shell_name) in &candidates {
        let rc_path = home.join(rc_name);
        if (rc_path.exists() || shell.contains(shell_name)) && !files.contains(&rc_path) {
            files.push(rc_path);
        }
    }

    // Fish uses a different config location
    let fish_config = home.join(".config/fish/config.fish");
    if fish_config.exists() || shell.contains("fish") {
        files.push(fish_config);
    }

    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_alias_line_format() {
        assert!(ALIAS_LINE.contains("alias git=oobo"));
        assert!(ALIAS_LINE.contains(ALIAS_MARKER));
    }

    #[test]
    fn test_install_and_uninstall_in_file() {
        let tmp = TempDir::new().unwrap();
        let rc = tmp.path().join(".zshrc");
        fs::write(&rc, "# existing content\n").unwrap();

        // Simulate install
        let content = fs::read_to_string(&rc).unwrap();
        let new_content = format!("{content}\n{ALIAS_LINE}\n");
        fs::write(&rc, &new_content).unwrap();

        let after_install = fs::read_to_string(&rc).unwrap();
        assert!(after_install.contains(ALIAS_MARKER));

        // Simulate uninstall
        let filtered: Vec<&str> = after_install
            .lines()
            .filter(|line| !line.contains(ALIAS_MARKER))
            .collect();
        fs::write(&rc, filtered.join("\n") + "\n").unwrap();

        let after_uninstall = fs::read_to_string(&rc).unwrap();
        assert!(!after_uninstall.contains(ALIAS_MARKER));
        assert!(after_uninstall.contains("existing content"));
    }
}
