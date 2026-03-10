/// A chat session discovered from any AI tool.
#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: String,
    pub name: String,
    pub mode: String,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub project_path: String,
    /// Retained for future workspace-aware features.
    #[allow(dead_code)]
    pub workspace_dir: String,
    pub source: String,
}

impl Session {
    pub fn sort_key(&self) -> i64 {
        self.updated_at.or(self.created_at).unwrap_or(0)
    }

    pub fn short_id(&self) -> &str {
        if self.session_id.len() >= 8 {
            &self.session_id[..8]
        } else {
            &self.session_id
        }
    }

    pub fn updated_at_iso(&self) -> String {
        epoch_ms_to_iso(self.updated_at)
    }

    pub fn created_at_iso(&self) -> String {
        epoch_ms_to_iso(self.created_at)
    }
}

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
        };
        assert_eq!(s.short_id(), "2c97dced");
    }
}
