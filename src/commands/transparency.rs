use crate::config::Config;

pub fn run(cfg: &Config, mode: Option<&str>) -> Result<(), String> {
    let root = crate::git::proxy::project_root(cfg).ok_or("not inside a git repository")?;

    let db = crate::db::Db::open().map_err(|e| format!("cannot open db: {e}"))?;
    let mut settings = db.get_project_settings_by_path(&root)?;

    match mode {
        Some("on") => {
            if settings.transparency.as_deref() == Some("on") {
                eprintln!("oobo: transparency is already on for {root}");
                return Ok(());
            }
            settings.transparency = Some("on".into());
            let project_id = resolve_project_id(&db, &root);
            ensure_project_exists(&db, &project_id, &root);
            db.set_project_settings(&project_id, &settings)?;
            eprintln!("oobo: transparency on for {root}");
            eprintln!("      redacted transcripts will sync alongside anchor metadata.");
            eprintln!("      run `oobo transparency off` to disable.");
        }
        Some("off") => {
            if settings.transparency.as_deref() == Some("off") {
                eprintln!("oobo: transparency is already off for {root}");
                return Ok(());
            }
            settings.transparency = Some("off".into());
            let project_id = resolve_project_id(&db, &root);
            ensure_project_exists(&db, &project_id, &root);
            db.set_project_settings(&project_id, &settings)?;
            eprintln!("oobo: transparency off for {root}");
            eprintln!("      transcripts stay local; anchor metadata still syncs.");
        }
        Some("reset") => {
            if settings.transparency.is_none() {
                eprintln!("oobo: {root} is already using the global default");
                return Ok(());
            }
            settings.transparency = None;
            let project_id = resolve_project_id(&db, &root);
            db.set_project_settings(&project_id, &settings)?;
            eprintln!("oobo: cleared per-repo transparency override for {root}");
            eprintln!("      now using global default ({})", cfg.transparency.mode);
        }
        _ => {
            let effective = settings
                .transparency
                .as_deref()
                .unwrap_or(&cfg.transparency.mode);
            let source = if settings.transparency.is_some() {
                "per-repo override"
            } else {
                "global default"
            };
            eprintln!("oobo: transparency is {effective} for {root} ({source})");
        }
    }

    Ok(())
}

pub fn run_list(cfg: &Config) {
    let mut found = false;

    if let Ok(db) = crate::db::Db::open() {
        if let Ok(projects) = db.list_projects() {
            for p in &projects {
                if let Ok(s) = db.get_project_settings(&p.id) {
                    if let Some(mode) = &s.transparency {
                        println!("{}\t{mode}", p.path);
                        found = true;
                    }
                }
            }
        }
    }

    if !found {
        eprintln!("no per-repo transparency overrides");
        eprintln!("global default: {}", cfg.transparency.mode);
    }
}

fn resolve_project_id(db: &crate::db::Db, path: &str) -> String {
    db.get_project_by_path(path)
        .ok()
        .flatten()
        .map(|p| p.id)
        .unwrap_or_else(|| crate::db::projects::path_to_project_id(path))
}

fn ensure_project_exists(db: &crate::db::Db, project_id: &str, path: &str) {
    if let Err(e) = db.ensure_project(project_id, path) {
        eprintln!("oobo: warning: could not register project: {e}");
    }
}
