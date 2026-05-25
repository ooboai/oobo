pub mod app;
mod data;
mod detail;
mod draw;
mod external;
pub mod format;
mod input;
pub mod setup;
pub(crate) mod status;
pub mod transcript;
mod types;

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use ratatui::DefaultTerminal;

pub const KEY_POLL: Duration = Duration::from_millis(50);

/// Reopen stdin from /dev/tty if it's not already a terminal.
/// This allows the TUI to read keyboard input even when the binary
/// is launched from a pipe (e.g. curl | bash -> exec oobo setup).
#[cfg(unix)]
pub fn ensure_tty_stdin() -> io::Result<()> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        use std::fs::File;
        use std::os::unix::io::IntoRawFd;
        let tty = File::open("/dev/tty")?;
        let tty_fd = tty.into_raw_fd();
        if tty_fd != 0 {
            unsafe {
                libc::dup2(tty_fd, 0);
                libc::close(tty_fd);
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn ensure_tty_stdin() -> io::Result<()> {
    Ok(())
}

pub fn init() -> io::Result<DefaultTerminal> {
    ensure_tty_stdin()?;
    ratatui::try_init()
}

pub fn restore() {
    ratatui::restore();
}

pub fn next_key(timeout: Duration) -> io::Result<Option<KeyEvent>> {
    if event::poll(timeout)? {
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                return Ok(Some(key));
            }
        }
    }
    Ok(None)
}

pub fn format_tokens(n: i64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1}B", n as f64 / 1_000_000_000.0)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_tokens_small() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(999), "999");
    }

    #[test]
    fn test_format_tokens_thousands() {
        assert_eq!(format_tokens(1_000), "1.0K");
        assert_eq!(format_tokens(1_500), "1.5K");
        assert_eq!(format_tokens(999_999), "1000.0K");
    }

    #[test]
    fn test_format_tokens_millions() {
        assert_eq!(format_tokens(1_000_000), "1.0M");
        assert_eq!(format_tokens(2_500_000), "2.5M");
    }

    #[test]
    fn test_format_tokens_billions() {
        assert_eq!(format_tokens(1_000_000_000), "1.0B");
        assert_eq!(format_tokens(3_700_000_000), "3.7B");
    }
}
