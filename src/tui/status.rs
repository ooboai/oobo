pub const KEYBOARD_GRAMMAR: &str = "\
Universal keybindings (same in every TUI screen):

  q            Quit the app
  Esc          Go back one level (quit if at root)
  /            Activate filter input
  ?            Toggle help overlay
  Enter        Drill into selected item
  j/k or ↑/↓  Navigate list
  g / G        Jump to top / bottom
  PgUp / PgDn  Page scroll
  r            Reload data
  Ctrl-C       Clear filter (if active) or quit

Transcript-specific:
  t / T        Jump to next / previous tool call
  [ / ]        Previous / next session
  n / N        Next / previous search match

Feed-specific:
  /            Live filter (type to narrow the list)
  t            Cycle time window (all → 24h → 7d → 30d)
  e            Toggle tracking on/off
  d            Inline diff view (committed anchors only)
  b            Blame file picker
  L            Goto selected (time-travel, auto-stashes)
";
