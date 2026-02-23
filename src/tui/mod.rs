pub mod dash;
pub mod sessions;
pub mod setup;

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::DefaultTerminal;

pub fn init() -> io::Result<DefaultTerminal> {
    Ok(ratatui::init())
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

pub fn key_code(timeout: Duration) -> io::Result<Option<KeyCode>> {
    Ok(next_key(timeout)?.map(|k| k.code))
}
