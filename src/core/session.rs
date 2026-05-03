/// A chat session discovered from any AI tool.
#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: String,
    /// Display name (e.g. "Refactor auth module"). Populated by adapters;
    /// used for future session listing/search features.
    pub name: String,
    /// Mode label from the tool (e.g. "agent", "composer", "ask").
    /// Carried for future filtering and analytics.
    pub mode: String,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub project_path: String,
    /// Workspace directory from the tool. Needed for multi-root
    /// workspace support (planned).
    pub workspace_dir: String,
    pub source: String,
    /// Parent session ID if this is a subagent session.
    pub parent_session_id: Option<String>,
    /// Subagent type (e.g. "explore", "shell", "generalPurpose").
    pub subagent_type: Option<String>,
}

impl Session {
    pub fn sort_key(&self) -> i64 {
        self.updated_at.or(self.created_at).unwrap_or(0)
    }

    #[cfg(test)]
    pub fn short_id(&self) -> &str {
        if self.session_id.len() >= 8 {
            &self.session_id[..8]
        } else {
            &self.session_id
        }
    }

    pub fn is_subagent(&self) -> bool {
        self.parent_session_id.is_some()
    }
}

#[cfg(test)]
fn epoch_ms_to_iso(ts: Option<i64>) -> String {
    match ts {
        Some(ts) => {
            let secs = crate::utils::to_epoch_secs(ts);
            match chrono::DateTime::from_timestamp(secs, 0) {
                Some(dt) => dt.format("%Y-%m-%dT%H:%M:%S").to_string(),
                None => String::new(),
            }
        }
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_epoch_ms_to_iso() {
        assert_eq!(epoch_ms_to_iso(None), "");
        assert_eq!(epoch_ms_to_iso(Some(1708617600000)), "2024-02-22T16:00:00");
    }

    #[test]
    fn test_session_short_id() {
        let s = Session {
            session_id: "2c97dced-3950-482e-b101-9eb7d1b18cf5".into(),
            name: "test".into(),
            mode: "agent".into(),
            created_at: Some(1000),
            updated_at: Some(2000),
            project_path: "/tmp".into(),
            workspace_dir: "/tmp".into(),
            source: "composer".into(),
            parent_session_id: None,
            subagent_type: None,
        };
        assert_eq!(s.short_id(), "2c97dced");
    }
}
