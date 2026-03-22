use crate::config::Config;
use crate::db::Db;

/// Show enriched commit history with anchor metadata.
///
/// `oobo anchors` shows recent commits annotated with linked sessions,
/// agent/human attribution, and token counts where available.
pub fn run(cfg: &Config, limit: usize, agent: bool) -> Result<(), String> {
    let db = Db::open()?;

    let commits = recent_commits_with_anchors(cfg, &db, limit)?;

    if agent {
        let j = serde_json::to_string_pretty(&commits).map_err(|e| format!("json: {e}"))?;
        println!("{j}");
        return Ok(());
    }

    if commits.is_empty() {
        println!("No commits with anchor metadata yet.");
        println!("Anchors are recorded automatically when you commit through oobo.");
        return Ok(());
    }

    for entry in &commits {
        let hash_short = if entry.commit_hash.len() >= 7 {
            &entry.commit_hash[..7]
        } else {
            &entry.commit_hash
        };

        let agent_indicator = if entry.sessions.is_empty() {
            "\x1b[90m●\x1b[0m"
        } else {
            "\x1b[35m◆\x1b[0m"
        };

        println!(
            "{} \x1b[33m{}\x1b[0m {}",
            agent_indicator, hash_short, entry.message,
        );

        if !entry.contributors.is_empty() {
            let contribs: Vec<String> = entry
                .contributors
                .iter()
                .map(|c| match c.role {
                    crate::core::anchor::ContributorRole::Human => c.name.clone(),
                    crate::core::anchor::ContributorRole::Agent => {
                        if let Some(ref m) = c.model {
                            format!("{} ({})", c.name, m)
                        } else {
                            c.name.clone()
                        }
                    }
                })
                .collect();
            println!("  \x1b[90mby: {}\x1b[0m", contribs.join(" + "));
        } else if !entry.author.is_empty() {
            println!("  \x1b[90mauthor: {}\x1b[0m", entry.author);
        }

        if entry.added > 0 || entry.deleted > 0 {
            let ai_pct = entry
                .ai_percentage
                .map(|p| format!("  \x1b[35m{:.0}% AI\x1b[0m", p))
                .unwrap_or_default();
            println!(
                "  \x1b[32m+{}\x1b[0m / \x1b[31m-{}\x1b[0m  ({} files){}",
                entry.added,
                entry.deleted,
                entry.files_changed.len(),
                ai_pct,
            );
            if entry.ai_added > 0 || entry.human_added > 0 {
                println!(
                    "  \x1b[90mai: +{}/-{}  human: +{}/-{}\x1b[0m",
                    entry.ai_added, entry.ai_deleted, entry.human_added, entry.human_deleted,
                );
            }
        }

        for sess in &entry.sessions {
            let model = sess.model.as_deref().unwrap_or("unknown");
            let tokens = match (sess.input_tokens, sess.output_tokens) {
                (Some(i), Some(o)) => format!(
                    " {}+{} tokens",
                    crate::tui::format_tokens(i as i64),
                    crate::tui::format_tokens(o as i64),
                ),
                _ => String::new(),
            };
            println!(
                "  \x1b[35m⤷ {} ({}, {}){}\x1b[0m",
                &sess.session_id[..sess.session_id.len().min(8)],
                sess.agent,
                model,
                tokens,
            );
        }

        if let Some(ref summary) = entry.summary {
            println!("  \x1b[36m{summary}\x1b[0m");
        }

        println!();
    }

    Ok(())
}

#[derive(Debug, serde::Serialize)]
struct LogEntry {
    commit_hash: String,
    message: String,
    author: String,
    author_type: crate::core::anchor::AuthorType,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    contributors: Vec<crate::core::anchor::Contributor>,
    branch: String,
    committed_at: i64,
    files_changed: Vec<String>,
    added: u32,
    deleted: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    file_changes: Vec<crate::core::anchor::FileChange>,
    #[serde(default)]
    ai_added: u32,
    #[serde(default)]
    ai_deleted: u32,
    #[serde(default)]
    human_added: u32,
    #[serde(default)]
    human_deleted: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    ai_percentage: Option<f64>,
    sessions: Vec<SessionEntry>,
    summary: Option<String>,
    intent: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct SessionEntry {
    session_id: String,
    agent: String,
    model: Option<String>,
    link_type: String,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    files_touched: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subagent_type: Option<String>,
}

fn recent_commits_with_anchors(
    cfg: &Config,
    db: &Db,
    limit: usize,
) -> Result<Vec<LogEntry>, String> {
    let git_log = crate::git::proxy::run_git_capture(
        cfg,
        &[
            "log",
            &format!("-{limit}"),
            "--format=%H|||%s|||%an <%ae>|||%aI",
        ],
    )?;

    let mut entries = Vec::new();

    for line in git_log.lines() {
        let parts: Vec<&str> = line.splitn(4, "|||").collect();
        if parts.len() < 4 {
            continue;
        }

        let hash = parts[0].to_string();
        let message = parts[1].to_string();
        let author = parts[2].to_string();
        let _date = parts[3];

        let anchor = load_anchor_from_db(db, &hash);

        let mut entry = LogEntry {
            commit_hash: hash,
            message,
            author,
            author_type: crate::core::anchor::AuthorType::Human,
            contributors: Vec::new(),
            branch: String::new(),
            committed_at: 0,
            files_changed: Vec::new(),
            added: 0,
            deleted: 0,
            file_changes: Vec::new(),
            ai_added: 0,
            ai_deleted: 0,
            human_added: 0,
            human_deleted: 0,
            ai_percentage: None,
            sessions: Vec::new(),
            summary: None,
            intent: None,
        };

        if let Some(anchor) = anchor {
            entry.author_type = anchor.author_type;
            entry.contributors = anchor.contributors;
            entry.branch = anchor.branch;
            entry.committed_at = anchor.committed_at;
            entry.files_changed = anchor.files_changed;
            entry.added = anchor.added;
            entry.deleted = anchor.deleted;
            entry.file_changes = anchor.file_changes;
            entry.ai_added = anchor.ai_added;
            entry.ai_deleted = anchor.ai_deleted;
            entry.human_added = anchor.human_added;
            entry.human_deleted = anchor.human_deleted;
            entry.ai_percentage = anchor.ai_percentage;
            entry.summary = anchor.summary;
            entry.intent = anchor.intent;

            for sid in &anchor.session_ids {
                if let Some(link) = load_session_link(db, &entry.commit_hash, sid) {
                    entry.sessions.push(link);
                } else {
                    entry.sessions.push(SessionEntry {
                        session_id: sid.clone(),
                        agent: String::new(),
                        model: None,
                        link_type: "unknown".to_string(),
                        input_tokens: None,
                        output_tokens: None,
                        files_touched: None,
                        parent_session_id: None,
                        subagent_type: None,
                    });
                }
            }
        }

        entries.push(entry);
    }

    Ok(entries)
}

fn load_anchor_from_db(db: &Db, commit_hash: &str) -> Option<crate::core::anchor::Anchor> {
    let raw: String = db
        .conn
        .query_row(
            "SELECT raw_json FROM anchors WHERE commit_hash = ?1",
            [commit_hash],
            |row| row.get(0),
        )
        .ok()?;

    serde_json::from_str(&raw).ok()
}

fn load_session_link(db: &Db, commit_hash: &str, session_id: &str) -> Option<SessionEntry> {
    db.conn
        .query_row(
            "SELECT agent, model, link_type, input_tokens, output_tokens, files_touched,
                    parent_session_id, subagent_type
             FROM anchor_sessions
             WHERE commit_hash = ?1 AND session_id = ?2",
            rusqlite::params![commit_hash, session_id],
            |row| {
                let ft_json: Option<String> = row.get(5)?;
                let files_touched: Option<Vec<String>> =
                    ft_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(SessionEntry {
                    session_id: session_id.to_string(),
                    agent: row.get(0)?,
                    model: row.get(1)?,
                    link_type: row.get(2)?,
                    input_tokens: row.get(3)?,
                    output_tokens: row.get(4)?,
                    files_touched,
                    parent_session_id: row.get(6)?,
                    subagent_type: row.get(7)?,
                })
            },
        )
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_entry_serialize() {
        let entry = LogEntry {
            commit_hash: "abc123def".into(),
            message: "feat: add login".into(),
            author: "dev <dev@x.com>".into(),
            author_type: crate::core::anchor::AuthorType::Assisted,
            contributors: vec![
                crate::core::anchor::Contributor {
                    name: "dev".into(),
                    role: crate::core::anchor::ContributorRole::Human,
                    model: None,
                },
                crate::core::anchor::Contributor {
                    name: "cursor".into(),
                    role: crate::core::anchor::ContributorRole::Agent,
                    model: Some("claude-sonnet-4-20250514".into()),
                },
            ],
            branch: "main".into(),
            committed_at: 1700000000,
            files_changed: vec!["auth.rs".into()],
            added: 50,
            deleted: 5,
            file_changes: vec![crate::core::anchor::FileChange {
                path: "auth.rs".into(),
                added: 50,
                deleted: 5,
                attribution: Some(crate::core::anchor::FileAttribution::Ai),
                agent: Some("cursor".into()),
            }],
            ai_added: 50,
            ai_deleted: 5,
            human_added: 0,
            human_deleted: 0,
            ai_percentage: Some(100.0),
            sessions: vec![],
            summary: Some("Added login".into()),
            intent: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("abc123def"));
        assert!(json.contains("feat: add login"));
        assert!(json.contains("ai_percentage"));
        assert!(json.contains("ai_added"));
        assert!(json.contains("file_changes"));
    }

    #[test]
    fn test_session_entry_serialize() {
        let entry = SessionEntry {
            session_id: "sess-abc".into(),
            agent: "cursor".into(),
            model: None,
            link_type: "explicit".into(),
            input_tokens: None,
            output_tokens: None,
            files_touched: Some(vec!["src/main.rs".into()]),
            parent_session_id: None,
            subagent_type: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"model\":null"));
        assert!(json.contains("files_touched"));
    }
}
