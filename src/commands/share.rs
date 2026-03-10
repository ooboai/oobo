use serde::Serialize;

use crate::config::Config;
use crate::session;

#[derive(Debug, Serialize)]
struct SharedSession {
    session_id: String,
    source: String,
    model: Option<String>,
    messages: Vec<SharedMessage>,
    stats: Option<SharedStats>,
    shared_at: String,
    oobo_version: String,
}

#[derive(Debug, Serialize)]
struct SharedMessage {
    role: String,
    text: String,
}

#[derive(Debug, Serialize)]
struct SharedStats {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    duration_secs: Option<i64>,
}

pub fn run(
    cfg: &Config,
    session_id: &str,
    output: Option<String>,
    agent: bool,
) -> Result<(), String> {
    let session = session::find_session_any(session_id)?;

    let transcript_path = session::find_transcript_path(&session);
    let mut messages = match &transcript_path {
        Some(path) => session::parse_messages(path, &session.source),
        None => Vec::new(),
    };

    if messages.is_empty() {
        messages = session::parse_messages_for_session(
            &session.project_path,
            &session.session_id,
            &session.source,
        );
    }

    if messages.is_empty() {
        return Err(format!(
            "no transcript found for session {}",
            &session.session_id[..session.session_id.len().min(8)]
        ));
    }

    let redacted_messages: Vec<SharedMessage> = messages
        .iter()
        .map(|m| SharedMessage {
            role: m.role.clone(),
            text: crate::redact::redact(&m.text),
        })
        .collect();

    let db = crate::db::Db::open().ok();
    let stats = db.as_ref().and_then(|db| {
        db.get_stats(&session.session_id, &session.source)
            .ok()
            .flatten()
            .map(|s| SharedStats {
                input_tokens: s.input_tokens,
                output_tokens: s.output_tokens,
                duration_secs: s.duration_secs,
            })
    });

    let shared = SharedSession {
        session_id: session.session_id.clone(),
        source: session.source.clone(),
        model: stats_model(&db, &session.session_id, &session.source),
        messages: redacted_messages,
        stats,
        shared_at: chrono::Utc::now().to_rfc3339(),
        oobo_version: env!("CARGO_PKG_VERSION").to_string(),
    };

    let json_str = serde_json::to_string_pretty(&shared).map_err(|e| format!("serialize: {e}"))?;

    if let Some(ref path) = output {
        std::fs::write(path, &json_str).map_err(|e| format!("write {path}: {e}"))?;
        if !agent {
            eprintln!(
                "shared session {} → {}",
                &session.session_id[..session.session_id.len().min(8)],
                path
            );
        }
    }

    if !cfg.server.api_key.is_empty() && output.is_none() {
        return upload_share(cfg, &json_str, agent);
    }

    if output.is_none() {
        if agent {
            println!("{json_str}");
        } else {
            eprintln!(
                "session {} redacted and ready ({} messages)",
                &session.session_id[..session.session_id.len().min(8)],
                shared.messages.len()
            );
            eprintln!("use --out <file> to save, or configure auth to upload");
        }
    }

    if let (true, Some(out_path)) = (agent, &output) {
        let resp = serde_json::json!({
            "status": "saved",
            "session_id": session.session_id,
            "path": out_path,
            "messages": shared.messages.len(),
        });
        crate::utils::print_json(&resp);
    }

    Ok(())
}

fn stats_model(db: &Option<crate::db::Db>, session_id: &str, source: &str) -> Option<String> {
    db.as_ref()
        .and_then(|db| db.get_stats(session_id, source).ok())
        .and_then(|s| s)
        .and_then(|s| s.model)
}

fn upload_share(cfg: &Config, json_body: &str, agent: bool) -> Result<(), String> {
    let url = format!("{}/api/v1/shares", cfg.server.url.trim_end_matches('/'));

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", cfg.server.api_key))
        .header("Content-Type", "application/json")
        .header("User-Agent", format!("oobo/{}", env!("CARGO_PKG_VERSION")))
        .body(json_body.to_string())
        .send()
        .map_err(|e| format!("upload failed: {e}"))?;

    let status = resp.status();
    let body = resp.text().unwrap_or_default();

    if !status.is_success() {
        return Err(format!("server returned HTTP {status}: {body}"));
    }

    if agent {
        println!("{body}");
    } else if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body) {
        if let Some(share_url) = parsed.get("url").and_then(|u| u.as_str()) {
            eprintln!("shared: {share_url}");
        } else {
            eprintln!("shared successfully");
        }
    } else {
        eprintln!("shared successfully");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shared_session_serializes_all_fields() {
        let shared = SharedSession {
            session_id: "abc-123".into(),
            source: "cursor".into(),
            model: Some("claude-opus".into()),
            messages: vec![
                SharedMessage {
                    role: "user".into(),
                    text: "hello".into(),
                },
                SharedMessage {
                    role: "assistant".into(),
                    text: "hi there".into(),
                },
            ],
            stats: Some(SharedStats {
                input_tokens: Some(100),
                output_tokens: Some(200),
                duration_secs: Some(30),
            }),
            shared_at: "2025-01-01T00:00:00Z".into(),
            oobo_version: "0.1.0".into(),
        };

        let json_str = serde_json::to_string_pretty(&shared).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(v["session_id"], "abc-123");
        assert_eq!(v["source"], "cursor");
        assert_eq!(v["model"], "claude-opus");
        assert_eq!(v["shared_at"], "2025-01-01T00:00:00Z");
        assert_eq!(v["oobo_version"], "0.1.0");
        assert_eq!(v["messages"].as_array().unwrap().len(), 2);
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][0]["text"], "hello");
        assert_eq!(v["stats"]["input_tokens"], 100);
        assert_eq!(v["stats"]["output_tokens"], 200);
        assert_eq!(v["stats"]["duration_secs"], 30);
    }

    #[test]
    fn test_shared_session_serializes_without_optional_fields() {
        let shared = SharedSession {
            session_id: "def-456".into(),
            source: "claude".into(),
            model: None,
            messages: vec![SharedMessage {
                role: "user".into(),
                text: "test".into(),
            }],
            stats: None,
            shared_at: "2025-06-01T00:00:00Z".into(),
            oobo_version: "0.2.0".into(),
        };

        let json_str = serde_json::to_string(&shared).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert!(v["model"].is_null());
        assert!(v["stats"].is_null());
        assert_eq!(v["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_empty_messages_returns_error() {
        let cfg = Config::load_or_default();
        let result = run(&cfg, "nonexistent-session-id-xyz", None, false);
        assert!(result.is_err());
    }

    #[test]
    fn test_redacted_messages_replace_secrets() {
        let prefix = "sk_live_";
        let suffix = "abcdefghijklmnopqrstuvwxyz1234";
        let fake_key = format!("{prefix}{suffix}");
        let secret_text = format!("my token = {fake_key}");
        let redacted = crate::redact::redact(&secret_text);
        assert!(!redacted.contains(&fake_key));

        let msg = SharedMessage {
            role: "user".into(),
            text: redacted,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains(prefix));
    }

    #[test]
    fn test_shared_stats_partial_none() {
        let stats = SharedStats {
            input_tokens: Some(500),
            output_tokens: None,
            duration_secs: None,
        };
        let json = serde_json::to_string(&stats).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["input_tokens"], 500);
        assert!(v["output_tokens"].is_null());
        assert!(v["duration_secs"].is_null());
    }

    #[test]
    fn test_shared_session_json_roundtrip() {
        let shared = SharedSession {
            session_id: "round-trip-id".into(),
            source: "gemini".into(),
            model: Some("gemini-2.0-flash".into()),
            messages: vec![
                SharedMessage {
                    role: "user".into(),
                    text: "explain Rust".into(),
                },
                SharedMessage {
                    role: "assistant".into(),
                    text: "Rust is a systems language".into(),
                },
                SharedMessage {
                    role: "user".into(),
                    text: "thanks".into(),
                },
            ],
            stats: Some(SharedStats {
                input_tokens: Some(1500),
                output_tokens: Some(3200),
                duration_secs: Some(45),
            }),
            shared_at: "2026-03-08T12:00:00Z".into(),
            oobo_version: env!("CARGO_PKG_VERSION").to_string(),
        };

        let json_str = serde_json::to_string_pretty(&shared).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert!(v.is_object());
        assert_eq!(v["messages"].as_array().unwrap().len(), 3);
        assert_eq!(v["messages"][2]["role"], "user");
        assert_eq!(v["stats"]["input_tokens"], 1500);
        assert_eq!(v["source"], "gemini");
    }

    #[test]
    fn test_shared_session_empty_messages_vec() {
        let shared = SharedSession {
            session_id: "empty-msgs".into(),
            source: "cursor".into(),
            model: None,
            messages: vec![],
            stats: None,
            shared_at: "2026-01-01T00:00:00Z".into(),
            oobo_version: "0.1.0".into(),
        };
        let json = serde_json::to_string(&shared).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["messages"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_stats_model_returns_none_without_db() {
        let result = stats_model(&None, "some-id", "cursor");
        assert!(result.is_none());
    }
}
