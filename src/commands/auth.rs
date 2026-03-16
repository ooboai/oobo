use crate::cli::AuthAction;
use crate::config::Config;

pub fn run(action: AuthAction) -> Result<(), String> {
    let mut cfg = Config::load_or_default();

    match action {
        AuthAction::Login { key } => {
            let api_key = match key {
                Some(k) => k,
                None => prompt_key()?,
            };
            cfg.server.api_key = api_key;
            cfg.save()?;

            match verify_auth(&cfg) {
                Ok(info) => eprintln!("logged in: {info}"),
                Err(e) => {
                    eprintln!("warning: key saved but verification failed: {e}");
                    eprintln!("the key will still be used for requests");
                }
            }
        }
        AuthAction::Logout => {
            if cfg.server.api_key.is_empty() {
                eprintln!("not logged in");
            } else {
                cfg.server.api_key = String::new();
                cfg.save()?;
                eprintln!("logged out (API key removed)");
            }
        }
        AuthAction::Status => {
            show_status(&cfg);
        }
        AuthAction::SetRemote { url } => {
            cfg.server.url = url.clone();
            cfg.save()?;
            eprintln!("remote server set to: {url}");
        }
        AuthAction::Anthropic { key } => {
            cfg.claude.api_key = key;
            cfg.save()?;
            eprintln!("Anthropic Admin API key saved.");
        }
        AuthAction::Copilot { token } => {
            cfg.copilot.api_key = token;
            cfg.save()?;
            eprintln!("GitHub Copilot PAT saved.");
        }
        AuthAction::Windsurf { key } => {
            cfg.windsurf.api_key = key;
            cfg.save()?;
            eprintln!("Windsurf service key saved.");
        }
        AuthAction::Openai { key } => {
            cfg.codex.api_key = key;
            cfg.save()?;
            eprintln!("OpenAI API key saved.");
        }
        AuthAction::Google { key } => {
            cfg.gemini.api_key = key;
            cfg.save()?;
            eprintln!("Google AI Studio API key saved.");
        }
        AuthAction::Show => {
            show_status(&cfg);
        }
    }

    Ok(())
}

fn prompt_key() -> Result<String, String> {
    eprint!("API key: ");
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("read stdin: {e}"))?;
    let trimmed = line.trim().to_string();
    if trimmed.is_empty() {
        return Err("no key provided".into());
    }
    Ok(trimmed)
}

fn verify_auth(cfg: &Config) -> Result<String, String> {
    let url = format!("{}/anchors/verify", cfg.server.url.trim_end_matches('/'));

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", cfg.server.api_key))
        .header("User-Agent", format!("oobo/{}", env!("CARGO_PKG_VERSION")))
        .send()
        .map_err(|e| format!("connection failed: {e}"))?;

    if resp.status().is_success() {
        let body = resp.text().unwrap_or_default();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(email) = v.get("email").and_then(|e| e.as_str()) {
                return Ok(email.to_string());
            }
        }
        Ok("authenticated".into())
    } else {
        Err(format!("HTTP {}", resp.status()))
    }
}

fn show_status(cfg: &Config) {
    eprintln!("remote: {}", cfg.server.url);
    if cfg.server.api_key.is_empty() {
        eprintln!("auth:   not logged in");
    } else {
        eprintln!("auth:   {} (key set)", mask_key(&cfg.server.api_key));
    }

    eprintln!();
    eprintln!("tool keys:");
    let tools = [
        ("Anthropic", &cfg.claude.api_key),
        ("Copilot", &cfg.copilot.api_key),
        ("Windsurf", &cfg.windsurf.api_key),
        ("OpenAI", &cfg.codex.api_key),
        ("Google", &cfg.gemini.api_key),
        ("Cursor", &cfg.cursor.api_key),
    ];

    for (name, key) in &tools {
        if key.is_empty() {
            eprintln!("  {name:<12} (not set)");
        } else {
            eprintln!("  {name:<12} {}", mask_key(key));
        }
    }
}

fn mask_key(key: &str) -> String {
    if key.len() <= 8 {
        "••••••••".to_string()
    } else {
        format!("{}••••{}", &key[..4], &key[key.len() - 4..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_key_long() {
        let fake = format!("{}1234567890abcdef", "sk_live_");
        let masked = mask_key(&fake);
        assert_eq!(masked, "sk_l••••cdef");
    }

    #[test]
    fn test_mask_key_short() {
        let masked = mask_key("abc123");
        assert_eq!(masked, "••••••••");
    }

    #[test]
    fn test_mask_key_exactly_8_chars() {
        let masked = mask_key("12345678");
        assert_eq!(masked, "••••••••");
    }

    #[test]
    fn test_mask_key_9_chars() {
        let masked = mask_key("123456789");
        assert_eq!(masked, "1234••••6789");
    }

    #[test]
    fn test_mask_key_empty() {
        let masked = mask_key("");
        assert_eq!(masked, "••••••••");
    }

    #[test]
    fn test_mask_key_preserves_prefix_suffix() {
        let fake = format!("{}{}", "AKIA", "1234ABCD5678WXYZ");
        let masked = mask_key(&fake);
        assert!(masked.starts_with("AKIA"));
        assert!(masked.ends_with("WXYZ"));
        assert!(masked.contains("••••"));
    }

    #[test]
    fn test_mask_key_single_char() {
        let masked = mask_key("x");
        assert_eq!(masked, "••••••••");
    }

    #[test]
    fn test_mask_key_very_long() {
        let key = "a".repeat(200);
        let masked = mask_key(&key);
        assert!(masked.starts_with("aaaa"));
        assert!(masked.ends_with("aaaa"));
        assert_eq!(masked.len(), 4 + "••••".len() + 4);
    }

    #[test]
    fn test_show_status_not_logged_in() {
        let cfg = Config {
            server: crate::config::ServerConfig {
                url: "https://example.com".into(),
                api_key: String::new(),
                sync: false,
            },
            ..Config::load_or_default()
        };
        show_status(&cfg);
    }

    #[test]
    fn test_show_status_logged_in() {
        let mut cfg = Config::load_or_default();
        cfg.server.url = "https://test.example.com".into();
        cfg.server.api_key = "test_key_abcdefghijklmnop".into();
        show_status(&cfg);
    }
}
