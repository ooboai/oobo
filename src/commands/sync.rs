use crate::config::Config;

pub fn resolve_api_key(cfg: &Config) -> String {
    if let Ok(key) = std::env::var("OOBO_SECRET_KEY") {
        if !key.is_empty() {
            return key;
        }
    }
    cfg.server.api_key.clone()
}

pub fn auto_hydrate(project_root: &str) {
    if !crate::git::orphan::branch_exists(project_root) {
        let _ = crate::git::orphan::fetch_and_reconcile(project_root);
    }
}
