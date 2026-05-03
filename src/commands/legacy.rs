//! Legacy command hints for the 0.1.x → 1.0.0 transition.
//!
//! A removed command is intercepted before clap parsing. In TTY mode we
//! offer to run the new command; in non-TTY we exit with a clear hint.
//!
//! This module is scheduled to be deleted in 1.1.0.

use std::io::{self, IsTerminal, Write};

/// A single legacy → new-command rewrite.
pub struct Hint {
    pub legacy: &'static str,
    pub message: &'static str,
    pub mapped: Option<&'static [&'static str]>,
}

const HINTS: &[Hint] = &[
    Hint {
        legacy: "a",
        message: "bare 'oobo' shows the memory feed now. use 'oobo anchor show <sha>' to drill in.",
        mapped: Some(&[]),
    },
    Hint {
        legacy: "alias",
        message: "removed. oobo is standalone — no git alias needed.",
        mapped: None,
    },
    Hint {
        legacy: "scan",
        message: "indexing is automatic now. for a forced reindex: oobo setup --reindex",
        mapped: Some(&["setup", "--reindex"]),
    },
    Hint {
        legacy: "index",
        message: "indexing is automatic now. for a forced reindex: oobo setup --reindex",
        mapped: Some(&["setup", "--reindex"]),
    },
    Hint {
        legacy: "sessions",
        message: "sessions are shown inside 'oobo anchor show <sha>' or 'oobo search'.",
        mapped: Some(&[]),
    },
    Hint {
        legacy: "projects",
        message: "manage projects via 'oobo setup'.",
        mapped: Some(&["setup"]),
    },
    Hint {
        legacy: "ignore",
        message: "use 'oobo disable' instead.",
        mapped: Some(&["disable"]),
    },
    Hint {
        legacy: "unignore",
        message: "use 'oobo enable' instead.",
        mapped: Some(&["enable"]),
    },
    Hint {
        legacy: "sync",
        message:
            "removed. team sync is git-first (orphan branch). for remote search set an API key: oobo settings set key <...>",
        mapped: Some(&["settings"]),
    },
    Hint {
        legacy: "transparency",
        message: "use 'oobo settings set transparency on|off' (advanced).",
        mapped: Some(&["settings"]),
    },
    Hint {
        legacy: "auth",
        message: "use 'oobo settings set key <your-key>'.",
        mapped: Some(&["settings"]),
    },
    Hint {
        legacy: "card",
        message: "removed in 1.0.",
        mapped: None,
    },
    Hint {
        legacy: "dash",
        message: "removed in 1.0; visit 'oobo' in a repo for the TUI.",
        mapped: Some(&[]),
    },
    Hint {
        legacy: "sources",
        message: "removed in 1.0; run 'oobo setup --repair' to re-detect tool paths.",
        mapped: Some(&["setup", "--repair"]),
    },
    Hint {
        legacy: "inspect",
        message: "removed in 1.0; run 'oobo setup --repair' for diagnostics.",
        mapped: Some(&["setup", "--repair"]),
    },
    Hint {
        legacy: "stats",
        message: "stats are inline in the oobo anchor view and in 'oobo anchor show <sha>'.",
        mapped: Some(&[]),
    },
    Hint {
        legacy: "agent",
        message: "removed; use the global flag '--agent' instead.",
        mapped: None,
    },
    Hint {
        legacy: "share",
        message: "removed; use 'oobo anchor show <sha> --json' for redacted output.",
        mapped: Some(&[]),
    },
    Hint {
        legacy: "export",
        message: "removed; use 'oobo anchor show <sha> --json'.",
        mapped: Some(&[]),
    },
    Hint {
        legacy: "version",
        message: "use 'oobo --version'.",
        mapped: None,
    },
    Hint {
        legacy: "doctor",
        message: "removed; run 'oobo setup --repair'.",
        mapped: Some(&["setup", "--repair"]),
    },
];

/// Look up a hint for `verb`. Returns `None` if there's no match.
pub fn lookup(verb: &str) -> Option<&'static Hint> {
    HINTS.iter().find(|h| h.legacy == verb)
}

/// Emit the hint and (when TTY + mapped) prompt to run the new command.
/// Returns the exit code to use, or `None` if the caller should continue
/// execution with the mapped args.
pub fn handle(hint: &Hint) -> Option<i32> {
    eprintln!("oobo: '{}' was removed in 1.0.", hint.legacy);
    eprintln!("      {}", hint.message);
    eprintln!("      (this hint will be removed in 1.1.0)");

    let stdin_is_tty = io::stdin().is_terminal();
    let stdout_is_tty = io::stdout().is_terminal();

    let mapped = match hint.mapped {
        Some(m) => m,
        None => return Some(2),
    };

    if !stdin_is_tty || !stdout_is_tty {
        return Some(2);
    }

    eprintln!();
    if mapped.is_empty() {
        eprint!("run 'oobo' now? [Y/n]: ");
    } else {
        eprint!("run 'oobo {}' now? [Y/n]: ", mapped.join(" "));
    }
    let _ = io::stderr().flush();

    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return Some(2);
    }
    let answer = line.trim().to_ascii_lowercase();
    if answer.is_empty() || answer == "y" || answer == "yes" {
        None
    } else {
        Some(2)
    }
}
