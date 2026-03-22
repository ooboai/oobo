const REPO: &str = "ooboai/oobo";
const USER_AGENT: &str = concat!("oobo/", env!("CARGO_PKG_VERSION"));

pub fn run(check_only: bool) -> Result<(), String> {
    let current_version = env!("CARGO_PKG_VERSION");
    eprintln!("current version: v{current_version}");

    let latest = fetch_latest_version()?;
    let latest_clean = latest.trim_start_matches('v');

    if latest_clean == current_version {
        eprintln!("already up to date");
        return Ok(());
    }

    eprintln!("new version available: v{latest_clean}");

    if check_only {
        eprintln!("run `oobo update` to install");
        return Ok(());
    }

    eprintln!("downloading...");
    install_latest(&latest)?;
    eprintln!("updated to v{latest_clean}");

    let current_exe = std::env::current_exe().ok();
    if let Some(exe) = current_exe {
        eprintln!("running post-update migrations...");
        match std::process::Command::new(&exe)
            .args(["update", "--post-update"])
            .status()
        {
            Ok(s) if !s.success() => eprintln!("warning: post-update exited with {s}"),
            Err(e) => eprintln!("warning: could not run post-update: {e}"),
            _ => {}
        }
    }

    Ok(())
}

pub fn run_post_update() -> Result<(), String> {
    eprintln!("oobo: running post-update tasks...");

    match crate::commands::agent::ensure_skill_file() {
        Ok(_) => eprintln!("  skill file updated"),
        Err(e) => eprintln!("  skill file: {e}"),
    }

    match crate::db::Db::open() {
        Ok(_) => eprintln!("  database migrations applied"),
        Err(e) => eprintln!("  database: {e}"),
    }

    let hooks = crate::hooks::install::install_all_agent_hooks();
    if hooks.is_empty() {
        eprintln!("  agent hooks: none installed");
    } else {
        eprintln!("  agent hooks: {}", hooks.join(", "));
    }

    Ok(())
}

fn fetch_latest_version() -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");

    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| format!("http client error: {e}"))?;

    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("cannot reach GitHub: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API returned {}", resp.status()));
    }

    let body: serde_json::Value = resp.json().map_err(|e| format!("invalid response: {e}"))?;

    body.get("tag_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "no tag_name in release".to_string())
}

fn install_latest(tag: &str) -> Result<(), String> {
    let target = current_target();
    if target == "unknown" || target.is_empty() {
        return Err("prebuilt binaries are not available for this platform".to_string());
    }
    let asset_name = format!("oobo-{tag}-{target}.tar.gz");
    let url = format!("https://github.com/{REPO}/releases/download/{tag}/{asset_name}");

    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| format!("http client error: {e}"))?;

    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("cannot download: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("download failed ({}): {asset_name}", resp.status()));
    }

    let bytes = resp
        .bytes()
        .map_err(|e| format!("cannot read download: {e}"))?;

    let current_exe =
        std::env::current_exe().map_err(|e| format!("cannot find current binary: {e}"))?;

    let tmp_dir = std::env::temp_dir().join(format!("oobo-update-{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("cannot create temp dir: {e}"))?;

    let archive_path = tmp_dir.join(&asset_name);
    std::fs::write(&archive_path, &bytes).map_err(|e| format!("cannot write archive: {e}"))?;

    let status = std::process::Command::new("tar")
        .args(["xzf", &archive_path.to_string_lossy()])
        .current_dir(&tmp_dir)
        .status()
        .map_err(|e| format!("cannot extract archive: {e}"))?;

    if !status.success() {
        return Err("tar extraction failed".to_string());
    }

    let new_binary = tmp_dir.join("oobo");
    if !new_binary.exists() {
        return Err("binary not found in archive".to_string());
    }

    let backup = current_exe.with_extension("old");
    if let Err(e) = std::fs::rename(&current_exe, &backup) {
        eprintln!("oobo: warning: could not backup current binary: {e}");
    }

    if let Err(e) = std::fs::copy(&new_binary, &current_exe) {
        let _ = std::fs::rename(&backup, &current_exe);
        return Err(format!("cannot replace binary: {e}"));
    }

    let _ = std::fs::remove_file(&backup);
    let _ = std::fs::remove_dir_all(&tmp_dir);

    Ok(())
}

fn current_target() -> String {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "aarch64-apple-darwin".into()
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "x86_64-apple-darwin".into()
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        let libc = if is_musl() { "musl" } else { "gnu" };
        format!("x86_64-unknown-linux-{libc}")
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        let libc = if is_musl() { "musl" } else { "gnu" };
        format!("aarch64-unknown-linux-{libc}")
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
    )))]
    {
        "unknown".into()
    }
}

#[cfg(target_os = "linux")]
fn is_musl() -> bool {
    std::path::Path::new("/etc/alpine-release").exists()
        || std::process::Command::new("ldd")
            .arg("--version")
            .output()
            .map(|o| {
                let out = String::from_utf8_lossy(&o.stderr);
                out.to_ascii_lowercase().contains("musl")
            })
            .unwrap_or(false)
}
