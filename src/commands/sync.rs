use crate::config::Config;

const HYDRATION_INTERVAL_SECS: i64 = 3600;

/// `oobo sync [on|off]` — toggle backend sync or show current status.
pub fn run(cfg: &mut Config, mode: Option<&str>) -> Result<(), String> {
    match mode {
        Some("on") => enable_sync(cfg),
        Some("off") => {
            cfg.server.sync = false;
            cfg.save()?;
            eprintln!("oobo: sync disabled");
            eprintln!("      anchor metadata is still written locally.");
            Ok(())
        }
        Some(other) => Err(format!("unknown mode: {other} (use on or off)")),
        None => {
            show_status(cfg);
            Ok(())
        }
    }
}

fn enable_sync(cfg: &mut Config) -> Result<(), String> {
    if cfg.server.api_key.is_empty() {
        eprintln!("oobo: no secret key found.");
        eprintln!();
        eprintln!("set via environment variable:");
        eprintln!("  export OOBO_SECRET_KEY=your_key_here");
        eprintln!();
        eprintln!("or enter it now (saved to ~/.oobo/config.toml):");
        eprint!("secret key: ");

        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| format!("read stdin: {e}"))?;
        let key = line.trim().to_string();
        if key.is_empty() {
            return Err("no key provided — sync not enabled".into());
        }
        cfg.server.api_key = key;
    }

    eprintln!();
    eprintln!("server: {}", cfg.server.url);
    eprint!("change server URL? [N/url]: ");

    let mut url_line = String::new();
    std::io::stdin()
        .read_line(&mut url_line)
        .map_err(|e| format!("read stdin: {e}"))?;
    let url_input = url_line.trim();
    if !url_input.is_empty() && url_input.to_lowercase() != "n" {
        cfg.server.url = url_input.to_string();
    }

    cfg.server.sync = true;
    cfg.save()?;

    eprintln!();
    eprintln!("oobo: sync enabled");
    eprintln!("      anchors will be pushed to {} on every commit.", cfg.server.url);
    eprintln!("      run `oobo sync off` to disable.");

    Ok(())
}

fn show_status(cfg: &Config) {
    if cfg.should_sync() {
        eprintln!("oobo: sync is on");
        eprintln!("      server: {}", cfg.server.url);
        eprintln!("      key:    {}", mask_key(&cfg.server.api_key));
    } else if cfg.server.sync && cfg.server.api_key.is_empty() {
        eprintln!("oobo: sync is on but no secret key is configured");
        eprintln!("      set OOBO_SECRET_KEY or run `oobo sync on` to provide one");
    } else {
        eprintln!("oobo: sync is off");
        eprintln!("      run `oobo sync on` to enable backend sync");
    }
}

fn mask_key(key: &str) -> String {
    if key.len() <= 8 {
        "••••••••".to_string()
    } else {
        format!("{}••••{}", &key[..4], &key[key.len() - 4..])
    }
}

/// `oobo sync --import` — pull anchors from the orphan branch into the local DB.
pub fn run_import(cfg: &Config) -> Result<(), String> {
    let root = crate::git::proxy::project_root(cfg).ok_or("not inside a git repository")?;
    let db = crate::db::Db::open()?;

    eprintln!("syncing anchors from orphan branch…");

    if let Err(e) = fetch_remote_branch(&root) {
        eprintln!("warning: could not fetch from remote: {e}");
    }

    let imported = crate::git::orphan::hydrate_from_branch(&root, &db)?;
    db.mark_hydrated(&root, imported)?;

    if imported > 0 {
        eprintln!("imported {imported} anchor(s) into local database");
    } else {
        eprintln!("local database is up to date");
    }

    Ok(())
}

/// Auto-sync: called from the interceptor or any project-aware command.
/// Non-blocking — only runs if overdue. Returns quietly on any error.
pub fn auto_hydrate(project_root: &str) {
    let db = match crate::db::Db::open() {
        Ok(db) => db,
        Err(_) => return,
    };

    if !db.needs_hydration(project_root, HYDRATION_INTERVAL_SECS) {
        return;
    }

    if !crate::git::orphan::branch_exists(project_root) {
        let _ = fetch_remote_branch(project_root);
        if !crate::git::orphan::branch_exists(project_root) {
            let _ = db.mark_hydrated(project_root, 0);
            return;
        }
    }

    match crate::git::orphan::hydrate_from_branch(project_root, &db) {
        Ok(n) => {
            let _ = db.mark_hydrated(project_root, n);
            if n > 0 {
                eprintln!("oobo: synced {n} anchor(s) from orphan branch");
            }
        }
        Err(e) => eprintln!("oobo: warning: hydration failed: {e}"),
    }
}

fn fetch_remote_branch(project_root: &str) -> Result<(), String> {
    let git = crate::config::find_real_git().unwrap_or_else(|| "git".into());
    let status = std::process::Command::new(git)
        .args(["fetch", "origin", "oobo/anchors/v1:oobo/anchors/v1"])
        .current_dir(project_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_QUARANTINE_PATH")
        .status()
        .map_err(|e| format!("failed to run git fetch: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "git fetch exited with code {}",
            status.code().unwrap_or(-1)
        ))
    }
}
