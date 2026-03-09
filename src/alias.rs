use std::fs;
use std::path::PathBuf;

use crate::cli::AliasAction;

const ALIAS_MARKER: &str = "# oobo alias";
const ALIAS_LINE_POSIX: &str = "alias git=oobo # oobo alias";
const ALIAS_LINE_FISH: &str = "alias git oobo # oobo alias";

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
            continue;
        }
        let line = if rc.to_string_lossy().contains("fish") {
            ALIAS_LINE_FISH
        } else {
            ALIAS_LINE_POSIX
        };
        let addition = format!("\n{line}\n");
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
        assert!(ALIAS_LINE_POSIX.contains("alias git=oobo"));
        assert!(ALIAS_LINE_POSIX.contains(ALIAS_MARKER));
        assert!(ALIAS_LINE_FISH.contains("alias git oobo"));
        assert!(ALIAS_LINE_FISH.contains(ALIAS_MARKER));
        assert!(!ALIAS_LINE_FISH.contains('='));
    }

    #[test]
    fn test_install_and_uninstall_in_file() {
        let tmp = TempDir::new().unwrap();
        let rc = tmp.path().join(".zshrc");
        fs::write(&rc, "# existing content\n").unwrap();

        let content = fs::read_to_string(&rc).unwrap();
        let new_content = format!("{content}\n{ALIAS_LINE_POSIX}\n");
        fs::write(&rc, &new_content).unwrap();

        let after_install = fs::read_to_string(&rc).unwrap();
        assert!(after_install.contains(ALIAS_MARKER));

        let filtered: Vec<&str> = after_install
            .lines()
            .filter(|line| !line.contains(ALIAS_MARKER))
            .collect();
        fs::write(&rc, filtered.join("\n") + "\n").unwrap();

        let after_uninstall = fs::read_to_string(&rc).unwrap();
        assert!(!after_uninstall.contains(ALIAS_MARKER));
        assert!(after_uninstall.contains("existing content"));
    }

    #[test]
    fn test_fish_path_selects_fish_syntax() {
        let rc_path = PathBuf::from("/home/user/.config/fish/config.fish");
        let line = if rc_path.to_string_lossy().contains("fish") {
            ALIAS_LINE_FISH
        } else {
            ALIAS_LINE_POSIX
        };
        assert_eq!(line, ALIAS_LINE_FISH);
        assert!(line.contains("alias git oobo"));
        assert!(!line.contains('='));
    }

    #[test]
    fn test_posix_path_selects_posix_syntax() {
        let rc_path = PathBuf::from("/home/user/.zshrc");
        let line = if rc_path.to_string_lossy().contains("fish") {
            ALIAS_LINE_FISH
        } else {
            ALIAS_LINE_POSIX
        };
        assert_eq!(line, ALIAS_LINE_POSIX);
        assert!(line.contains("alias git=oobo"));
    }

    #[test]
    fn test_bashrc_path_selects_posix_syntax() {
        let rc_path = PathBuf::from("/home/user/.bashrc");
        let line = if rc_path.to_string_lossy().contains("fish") {
            ALIAS_LINE_FISH
        } else {
            ALIAS_LINE_POSIX
        };
        assert_eq!(line, ALIAS_LINE_POSIX);
    }

    #[test]
    fn test_install_idempotent() {
        let tmp = TempDir::new().unwrap();
        let rc = tmp.path().join(".zshrc");
        fs::write(&rc, "# existing\n").unwrap();

        let content = fs::read_to_string(&rc).unwrap();
        assert!(!content.contains(ALIAS_MARKER));

        let new_content = format!("{content}\n{ALIAS_LINE_POSIX}\n");
        fs::write(&rc, &new_content).unwrap();

        let after_first = fs::read_to_string(&rc).unwrap();
        assert!(after_first.contains(ALIAS_MARKER));
        let count = after_first.matches(ALIAS_MARKER).count();
        assert_eq!(count, 1, "should have exactly one alias line");

        if !after_first.contains(ALIAS_MARKER) {
            let second = format!("{after_first}\n{ALIAS_LINE_POSIX}\n");
            fs::write(&rc, &second).unwrap();
        }

        let after_second = fs::read_to_string(&rc).unwrap();
        let count2 = after_second.matches(ALIAS_MARKER).count();
        assert_eq!(count2, 1, "idempotent: should still have one alias line");
    }

    #[test]
    fn test_uninstall_preserves_other_content() {
        let tmp = TempDir::new().unwrap();
        let rc = tmp.path().join(".bashrc");
        let original = "export PATH=/usr/local/bin:$PATH\nexport EDITOR=vim\n";
        let with_alias = format!("{original}{ALIAS_LINE_POSIX}\n");
        fs::write(&rc, &with_alias).unwrap();

        let content = fs::read_to_string(&rc).unwrap();
        let filtered: Vec<&str> = content
            .lines()
            .filter(|line| !line.contains(ALIAS_MARKER))
            .collect();
        fs::write(&rc, filtered.join("\n") + "\n").unwrap();

        let after = fs::read_to_string(&rc).unwrap();
        assert!(!after.contains(ALIAS_MARKER));
        assert!(after.contains("export PATH"));
        assert!(after.contains("export EDITOR=vim"));
    }
}
