use crate::config::Config;

/// All project-aware settings resolved in a single pass.
/// Avoids repeated disk reads of `.oobo/config`.
pub struct ResolvedConfig {
    pub api_key: String,
    pub api_url: String,
    pub anchor_remote: String,
}

impl ResolvedConfig {
    pub fn has_api_key(&self) -> bool {
        !self.api_key.is_empty()
    }
}

/// Resolve all project-aware settings in one disk read.
///
/// Precedence for each setting:
///   env var > `.oobo/config` (project) > `~/.oobo/config` (global) > compiled default
pub fn resolve(cfg: &Config, project_root: Option<&str>) -> ResolvedConfig {
    let project = project_root.and_then(|r| {
        crate::project_config::ProjectConfig::load(r)
            .ok()
            .flatten()
    });

    let api_key = resolve_api_key_inner(cfg, project.as_ref());
    let api_url = resolve_api_url_inner(cfg, project.as_ref());
    let anchor_remote = resolve_anchor_remote_inner(cfg, project.as_ref());

    ResolvedConfig {
        api_key,
        api_url,
        anchor_remote,
    }
}

fn resolve_api_key_inner(
    cfg: &Config,
    project: Option<&crate::project_config::ProjectConfig>,
) -> String {
    if let Ok(key) = std::env::var("OOBO_SECRET_KEY") {
        if !key.is_empty() {
            return key;
        }
    }
    if let Some(pcfg) = project {
        if !pcfg.server.api_key.is_empty() {
            return pcfg.server.api_key.clone();
        }
    }
    cfg.server.api_key.clone()
}

fn resolve_api_url_inner(
    cfg: &Config,
    project: Option<&crate::project_config::ProjectConfig>,
) -> String {
    if let Ok(url) = std::env::var("OOBO_API_URL") {
        if !url.is_empty() {
            return url;
        }
    }
    if let Some(pcfg) = project {
        if !pcfg.server.url.is_empty() {
            return pcfg.server.url.clone();
        }
    }
    cfg.server.url.clone()
}

fn resolve_anchor_remote_inner(
    cfg: &Config,
    project: Option<&crate::project_config::ProjectConfig>,
) -> String {
    if let Some(pcfg) = project {
        if !pcfg.anchors.remote.is_empty() {
            return pcfg.anchors.remote.clone();
        }
    }
    if !cfg.anchors.remote.is_empty() {
        return cfg.anchors.remote.clone();
    }
    crate::config::DEFAULT_ANCHOR_REMOTE.to_string()
}

pub fn auto_hydrate(project_root: &str) {
    if !crate::git::orphan::branch_exists(project_root) {
        let _ = crate::git::orphan::fetch_and_reconcile(project_root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Env vars are process-global; tests run in parallel. Guard all
    /// reads/writes behind a single mutex to prevent races.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        saved_key: Option<String>,
        saved_url: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn clean() -> Self {
            let lock = ENV_LOCK.lock().unwrap();
            let saved_key = std::env::var("OOBO_SECRET_KEY").ok();
            let saved_url = std::env::var("OOBO_API_URL").ok();
            std::env::remove_var("OOBO_SECRET_KEY");
            std::env::remove_var("OOBO_API_URL");
            Self { saved_key, saved_url, _lock: lock }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.saved_key {
                Some(v) => std::env::set_var("OOBO_SECRET_KEY", v),
                None => std::env::remove_var("OOBO_SECRET_KEY"),
            }
            match &self.saved_url {
                Some(v) => std::env::set_var("OOBO_API_URL", v),
                None => std::env::remove_var("OOBO_API_URL"),
            }
        }
    }

    #[test]
    fn resolve_loads_project_config_once() {
        let _env = EnvGuard::clean();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        let mut pcfg = crate::project_config::ProjectConfig::for_project("test");
        pcfg.server.api_key = "project_key".to_string();
        pcfg.server.url = "https://staging.example.com".to_string();
        pcfg.anchors.remote = "oobo-remote".to_string();
        pcfg.save(&root).unwrap();

        let mut cfg = Config::default();
        cfg.server.api_key = "global_key".to_string();
        cfg.anchors.remote = "global-remote".to_string();

        let resolved = resolve(&cfg, Some(&root));
        assert_eq!(resolved.api_key, "project_key");
        assert_eq!(resolved.api_url, "https://staging.example.com");
        assert_eq!(resolved.anchor_remote, "oobo-remote");
    }

    #[test]
    fn resolve_falls_back_to_global() {
        let _env = EnvGuard::clean();

        let mut cfg = Config::default();
        cfg.server.api_key = "global_key".to_string();
        cfg.anchors.remote = "global-remote".to_string();

        let resolved = resolve(&cfg, None);
        assert_eq!(resolved.api_key, "global_key");
        assert_eq!(resolved.api_url, crate::config::DEFAULT_SERVER_URL);
        assert_eq!(resolved.anchor_remote, "global-remote");
    }

    #[test]
    fn resolve_defaults_when_nothing_set() {
        let _env = EnvGuard::clean();

        let cfg = Config::default();
        let resolved = resolve(&cfg, None);
        assert!(resolved.api_key.is_empty());
        assert_eq!(resolved.api_url, crate::config::DEFAULT_SERVER_URL);
        assert_eq!(resolved.anchor_remote, crate::config::DEFAULT_ANCHOR_REMOTE);
    }

    #[test]
    fn env_overrides_everything() {
        let _env = EnvGuard::clean();
        std::env::set_var("OOBO_SECRET_KEY", "env_key");
        std::env::set_var("OOBO_API_URL", "https://env.example.com");

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        let mut pcfg = crate::project_config::ProjectConfig::for_project("test");
        pcfg.server.api_key = "project_key".to_string();
        pcfg.server.url = "https://project.example.com".to_string();
        pcfg.save(&root).unwrap();

        let cfg = Config::default();
        let resolved = resolve(&cfg, Some(&root));

        assert_eq!(resolved.api_key, "env_key");
        assert_eq!(resolved.api_url, "https://env.example.com");
    }
}
