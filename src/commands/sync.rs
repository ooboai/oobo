use crate::config::Config;

const HYDRATION_INTERVAL_SECS: i64 = 3600;

/// `oobo sync [on|off|key <key>]` — toggle backend sync or set per-project key.
pub fn run(cfg: &mut Config, mode: Option<&str>, extra: Option<&str>) -> Result<(), String> {
    match mode {
        Some("on") => enable_sync(cfg),
        Some("off") => disable_sync(cfg),
        Some("key") => set_project_key(cfg, extra),
        Some(other) => Err(format!("unknown mode: {other} (use on, off, or key <key>)")),
        None => {
            show_status(cfg);
            Ok(())
        }
    }
}

fn disable_sync(cfg: &mut Config) -> Result<(), String> {
    if let Some(project_root) = crate::git::proxy::project_root(cfg) {
        let db = crate::db::Db::open()?;
        let slug = crate::paths::slug_from_path(&project_root);
        let _ = db.ensure_project(&slug, &project_root);
        let mut settings = db.get_project_settings(&slug).unwrap_or_default();
        settings.sync = Some(false);
        db.set_project_settings(&slug, &settings)?;
        eprintln!("oobo: sync disabled for this project");
    } else {
        cfg.server.sync = false;
        cfg.save()?;
        eprintln!("oobo: sync disabled globally");
    }
    eprintln!("      anchor metadata is still written locally.");
    Ok(())
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

    cfg.save()?;

    if let Some(project_root) = crate::git::proxy::project_root(cfg) {
        let db = crate::db::Db::open()?;
        let slug = crate::paths::slug_from_path(&project_root);
        let _ = db.ensure_project(&slug, &project_root);
        let mut settings = db.get_project_settings(&slug).unwrap_or_default();
        settings.sync = Some(true);
        db.set_project_settings(&slug, &settings)?;

        eprintln!();
        eprintln!("oobo: sync enabled for this project");
    } else {
        cfg.server.sync = true;
        cfg.save()?;

        eprintln!();
        eprintln!("oobo: sync enabled globally");
    }

    eprintln!(
        "      anchors will be pushed to {} on every commit.",
        cfg.server.url
    );
    eprintln!("      run `oobo sync off` to disable.");

    Ok(())
}

fn show_status(cfg: &Config) {
    let project_sync = resolve_project_sync(cfg);
    let effective_key = resolve_api_key(cfg);
    let effective = project_sync.unwrap_or(cfg.server.sync) && !effective_key.is_empty();

    if effective {
        eprintln!("oobo: sync is on{}", scope_label(project_sync));
        eprintln!("      server: {}", cfg.server.url);
        eprintln!("      key:    {}", mask_key(&effective_key));
    } else if effective_key.is_empty() && (project_sync.unwrap_or(cfg.server.sync)) {
        eprintln!("oobo: sync is on but no secret key is configured");
        eprintln!("      set OOBO_SECRET_KEY or run `oobo sync on` to provide one");
    } else {
        eprintln!("oobo: sync is off{}", scope_label(project_sync));
        eprintln!("      run `oobo sync on` to enable backend sync");
    }
}

fn scope_label(project_sync: Option<bool>) -> &'static str {
    match project_sync {
        Some(_) => " (this project)",
        None => "",
    }
}

/// Check the per-project sync override. Returns `None` if no override exists
/// (fall back to global config).
pub fn resolve_project_sync(cfg: &Config) -> Option<bool> {
    let project_root = crate::git::proxy::project_root(cfg)?;
    let db = crate::db::Db::open().ok()?;
    let slug = crate::paths::slug_from_path(&project_root);
    let settings = db.get_project_settings(&slug).ok()?;
    settings.sync
}

/// Resolve per-project API key, falling back to global config.
pub fn resolve_api_key(cfg: &Config) -> String {
    if let Some(project_root) = crate::git::proxy::project_root(cfg) {
        if let Ok(db) = crate::db::Db::open() {
            let slug = crate::paths::slug_from_path(&project_root);
            if let Ok(settings) = db.get_project_settings(&slug) {
                if let Some(ref key) = settings.api_key {
                    if !key.is_empty() {
                        return key.clone();
                    }
                }
            }
        }
    }
    cfg.server.api_key.clone()
}

fn set_project_key(cfg: &Config, key_arg: Option<&str>) -> Result<(), String> {
    let project_root = crate::git::proxy::project_root(cfg)
        .ok_or("not inside a git repository — run this from a project directory")?;

    let key = if let Some(k) = key_arg {
        k.to_string()
    } else {
        eprint!("API key for this project: ");
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| format!("read stdin: {e}"))?;
        line.trim().to_string()
    };

    if key.is_empty() {
        return Err("no key provided".into());
    }

    let db = crate::db::Db::open()?;
    let slug = crate::paths::slug_from_path(&project_root);
    let _ = db.ensure_project(&slug, &project_root);
    let mut settings = db.get_project_settings(&slug).unwrap_or_default();
    settings.api_key = Some(key.clone());
    db.set_project_settings(&slug, &settings)?;

    eprintln!("oobo: API key set for this project");
    eprintln!("      key: {}", mask_key(&key));

    Ok(())
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

    if let Err(e) = crate::git::orphan::fetch_and_reconcile(&root) {
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
        let _ = crate::git::orphan::fetch_and_reconcile(project_root);
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

