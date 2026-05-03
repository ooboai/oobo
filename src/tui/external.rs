pub(super) fn suspend_and_run<F>(terminal: &mut ratatui::DefaultTerminal, f: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
{
    super::restore();
    let result = f();
    *terminal = super::init().map_err(|e| format!("tui init: {e}"))?;
    terminal.clear().ok();
    result
}

pub(super) fn run_oobo_blame(root: &str, file: &str, sha: &str) -> Result<(), String> {
    let oobo = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("oobo"));
    let status = std::process::Command::new(oobo)
        .args(["blame", file, sha])
        .current_dir(root)
        .status()
        .map_err(|e| format!("spawn oobo blame: {e}"))?;
    if !status.success() {
        return Err(format!("oobo blame exited {status}"));
    }
    println!("\npress enter to return...");
    let mut buf = String::new();
    let _ = std::io::stdin().read_line(&mut buf);
    Ok(())
}

pub(super) fn run_oobo_goto(root: &str, target_id: &str) -> Result<(), String> {
    let oobo = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("oobo"));
    let status = std::process::Command::new(oobo)
        .args(["goto", target_id])
        .current_dir(root)
        .status()
        .map_err(|e| format!("spawn oobo goto: {e}"))?;
    if !status.success() {
        return Err(format!("oobo goto exited {status}"));
    }
    println!("\npress enter to return...");
    let mut buf = String::new();
    let _ = std::io::stdin().read_line(&mut buf);
    Ok(())
}
