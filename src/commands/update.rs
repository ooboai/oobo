const REPO: &str = "ooboai/oobo";
const USER_AGENT: &str = concat!("oobo/", env!("CARGO_PKG_VERSION"));

pub async fn run(check_only: bool) -> Result<(), String> {
    let current_version = env!("CARGO_PKG_VERSION");
    eprintln!("current version: v{current_version}");

    let latest = fetch_latest_version().await?;
    let latest_clean = latest.trim_start_matches('v');

    if latest_clean == current_version || !is_newer(latest_clean, current_version) {
        eprintln!("already up to date");
        return Ok(());
    }

    eprintln!("new version available: v{latest_clean}");

    if check_only {
        eprintln!("run `oobo update` to install");
        return Ok(());
    }

    eprintln!("downloading...");
    install_latest(&latest).await?;
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

    let hooks = crate::hooks::install::install_all_agent_hooks();
    if hooks.is_empty() {
        eprintln!("  agent hooks: none installed");
    } else {
        eprintln!("  agent hooks: {}", hooks.join(", "));
    }

    Ok(())
}

async fn fetch_latest_version() -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");

    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| format!("http client error: {e}"))?;

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("cannot reach GitHub: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API returned {}", resp.status()));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| format!("invalid response: {e}"))?;

    body.get("tag_name")
        .and_then(|v| v.as_str())
        .map(std::string::ToString::to_string)
        .ok_or_else(|| "no tag_name in release".to_string())
}

async fn install_latest(tag: &str) -> Result<(), String> {
    let target = current_target();
    if target == "unknown" || target.is_empty() {
        return Err("prebuilt binaries are not available for this platform".to_string());
    }

    #[cfg(target_os = "windows")]
    let (asset_name, is_zip) = (format!("oobo-{tag}-{target}.zip"), true);
    #[cfg(not(target_os = "windows"))]
    let (asset_name, is_zip) = (format!("oobo-{tag}-{target}.tar.gz"), false);

    let url = format!("https://github.com/{REPO}/releases/download/{tag}/{asset_name}");

    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| format!("http client error: {e}"))?;

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("cannot download: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("download failed ({}): {asset_name}", resp.status()));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("cannot read download: {e}"))?;

    let current_exe =
        std::env::current_exe().map_err(|e| format!("cannot find current binary: {e}"))?;

    let tmp_dir = std::env::temp_dir().join(format!("oobo-update-{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("cannot create temp dir: {e}"))?;

    let archive_path = tmp_dir.join(&asset_name);
    std::fs::write(&archive_path, &bytes).map_err(|e| format!("cannot write archive: {e}"))?;

    if is_zip {
        extract_zip(&archive_path, &tmp_dir)?;
    } else {
        let status = std::process::Command::new("tar")
            .args(["xzf", &archive_path.to_string_lossy()])
            .current_dir(&tmp_dir)
            .status()
            .map_err(|e| format!("cannot extract archive: {e}"))?;
        if !status.success() {
            return Err("tar extraction failed".to_string());
        }
    }

    #[cfg(target_os = "windows")]
    let binary_name = "oobo.exe";
    #[cfg(not(target_os = "windows"))]
    let binary_name = "oobo";

    let new_binary = tmp_dir.join(binary_name);
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

#[cfg(target_os = "windows")]
fn extract_zip(archive: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("cannot open zip archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("cannot read zip archive: {e}"))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("zip entry error: {e}"))?;
        let name = entry.name().replace('\\', "/");
        let name = name.trim_start_matches('/');
        if name.contains("..") || name.is_empty() {
            continue;
        }
        let out_path = dest.join(name);
        if !out_path.starts_with(dest) {
            continue;
        }
        let mut out_file =
            std::fs::File::create(&out_path).map_err(|e| format!("cannot create file: {e}"))?;
        std::io::copy(&mut entry, &mut out_file)
            .map_err(|e| format!("cannot extract file: {e}"))?;
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
#[allow(clippy::unnecessary_wraps)]
fn extract_zip(_archive: &std::path::Path, _dest: &std::path::Path) -> Result<(), String> {
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
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "x86_64-pc-windows-msvc".into()
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        // No ARM64 Windows release artifact; x86_64 runs via emulation
        "x86_64-pc-windows-msvc".into()
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "aarch64"),
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

/// Compare two version strings, returning true if `candidate` is newer than `current`.
/// Strips pre-release suffixes (e.g. "-rc.1") for the numeric comparison,
/// then treats a pre-release of the same base version as older than the release.
fn is_newer(candidate: &str, current: &str) -> bool {
    fn parse_parts(v: &str) -> (Vec<u64>, &str) {
        let (base, pre) = v.split_once('-').map_or((v, ""), |(b, p)| (b, p));
        let nums: Vec<u64> = base.split('.').filter_map(|s| s.parse().ok()).collect();
        (nums, pre)
    }

    let (cand_nums, _cand_pre) = parse_parts(candidate);
    let (curr_nums, _curr_pre) = parse_parts(current);

    let max_len = cand_nums.len().max(curr_nums.len());
    for i in 0..max_len {
        let c = cand_nums.get(i).copied().unwrap_or(0);
        let r = curr_nums.get(i).copied().unwrap_or(0);
        if c > r {
            return true;
        }
        if c < r {
            return false;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn newer_major() {
        assert!(is_newer("2.0.0", "1.0.0"));
    }

    #[test]
    fn older_release() {
        assert!(!is_newer("0.1.15", "1.0.0-rc.1"));
    }

    #[test]
    fn same_version() {
        assert!(!is_newer("1.0.0", "1.0.0"));
    }

    #[test]
    fn newer_patch() {
        assert!(is_newer("1.0.1", "1.0.0"));
    }

    #[test]
    fn rc_not_newer_than_same_base() {
        assert!(!is_newer("1.0.0-rc.2", "1.0.0-rc.1"));
    }
}
