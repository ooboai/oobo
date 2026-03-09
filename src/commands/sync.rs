use crate::config::Config;

const HYDRATION_INTERVAL_SECS: i64 = 3600;

/// `oobo sync` — pull anchors from the orphan branch into the local DB.
pub fn run(cfg: &Config) -> Result<(), String> {
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
