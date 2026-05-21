/// Git subcommands that modify repository state.
/// When these succeed, oobo collects context and sends to the dashboard.
const WRITE_OPS: &[&str] = &[
    "commit",
    "push",
    "pull",
    "merge",
    "rebase",
    "cherry-pick",
    "revert",
    "reset",
    "stash",
    "tag",
];

/// Git top-level flags that consume the next argument as a value.
const FLAGS_WITH_VALUE: &[&str] = &["-C", "-c", "--git-dir", "--work-tree", "--namespace"];

/// Returns true if the first positional arg is a write operation.
pub fn is_write_op(args: &[&str]) -> bool {
    subcommand_name(args).is_some_and(|cmd| WRITE_OPS.contains(&cmd))
}

/// Extract the git subcommand name (first positional argument, skipping flags).
pub fn subcommand_name<'a>(args: &'a [&'a str]) -> Option<&'a str> {
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if FLAGS_WITH_VALUE.contains(arg) {
            skip_next = true;
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        return Some(arg);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_ops() {
        assert!(is_write_op(&["commit", "-m", "hello"]));
        assert!(is_write_op(&["push", "origin", "main"]));
        assert!(is_write_op(&["merge", "feature"]));
        assert!(is_write_op(&["rebase", "main"]));
        assert!(is_write_op(&["cherry-pick", "abc123"]));
        assert!(is_write_op(&["stash"]));
        assert!(is_write_op(&["tag", "v1.0"]));
    }

    #[test]
    fn test_read_ops() {
        assert!(!is_write_op(&["status"]));
        assert!(!is_write_op(&["log", "--oneline"]));
        assert!(!is_write_op(&["diff"]));
        assert!(!is_write_op(&["branch"]));
        assert!(!is_write_op(&["show", "HEAD"]));
    }

    #[test]
    fn test_flags_before_command() {
        assert!(is_write_op(&["-C", "/tmp", "commit", "-m", "x"]));
    }

    #[test]
    fn test_empty_args() {
        assert!(!is_write_op(&[]));
    }

    #[test]
    fn test_subcommand_name() {
        assert_eq!(subcommand_name(&["commit", "-m", "x"]), Some("commit"));
        assert_eq!(subcommand_name(&["-C", "/tmp", "status"]), Some("status"));
        assert_eq!(subcommand_name(&[]), None);
    }
}
