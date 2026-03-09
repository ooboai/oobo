use crate::config::Config;

pub fn run_ignore(cfg: &Config) -> Result<(), String> {
    let root = crate::git::proxy::project_root(cfg).ok_or("not inside a git repository")?;

    let db = crate::db::Db::open().map_err(|e| format!("cannot open db: {e}"))?;
    let mut settings = db.get_project_settings_by_path(&root)?;

    if settings.ignored {
        eprintln!("oobo: already ignoring {root}");
        return Ok(());
    }

    settings.ignored = true;
    let project = db.get_project_by_path(&root);
    let project_id = project
        .ok()
        .flatten()
        .map(|p| p.id)
        .unwrap_or_else(|| crate::db::projects::path_to_project_id(&root));
    ensure_project_exists(&db, &project_id, &root);
    db.set_project_settings(&project_id, &settings)?;

    eprintln!("oobo: stopped enriching {root}");
    eprintln!("      commits will not include AI attribution or anchor metadata.");
    eprintln!("      run `oobo unignore` to re-enable.");
    Ok(())
}

pub fn run_unignore(cfg: &Config) -> Result<(), String> {
    let root = crate::git::proxy::project_root(cfg).ok_or("not inside a git repository")?;

    let db = crate::db::Db::open().map_err(|e| format!("cannot open db: {e}"))?;
    let mut settings = db.get_project_settings_by_path(&root)?;

    if !settings.ignored {
        eprintln!("oobo: {root} is not ignored");
        return Ok(());
    }

    settings.ignored = false;
    let project = db.get_project_by_path(&root);
    let project_id = project
        .ok()
        .flatten()
        .map(|p| p.id)
        .unwrap_or_else(|| crate::db::projects::path_to_project_id(&root));
    db.set_project_settings(&project_id, &settings)?;

    eprintln!("oobo: re-enabled enrichment for {root}");
    Ok(())
}

pub fn run_list(cfg: &Config) {
    let mut found = false;

    // Show projects ignored via per-project DB settings
    if let Ok(db) = crate::db::Db::open() {
        if let Ok(projects) = db.list_projects() {
            for p in &projects {
                if let Ok(s) = db.get_project_settings(&p.id) {
                    if s.ignored {
                        println!("{}", p.path);
                        found = true;
                    }
                }
            }
        }
    }

    // Also show legacy global config entries
    for repo in &cfg.ignored_repos {
        println!("{repo}");
        found = true;
    }

    if !found {
        eprintln!("no ignored repos");
    }
}

fn ensure_project_exists(db: &crate::db::Db, project_id: &str, path: &str) {
    let name = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let now = chrono::Utc::now().timestamp();
    if let Err(e) = db.upsert_project(&crate::db::projects::ProjectRow {
        id: project_id.to_string(),
        path: path.to_string(),
        name,
        git_remote: None,
        discovered_at: now,
        last_seen_at: now,
        last_scanned_at: 0,
        tools: vec![],
    }) {
        eprintln!("oobo: warning: could not register project: {e}");
    }
}
