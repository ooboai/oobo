#![allow(dead_code)]

use std::process::{Command, Stdio};

const REDACT_PLACEHOLDER: &str = "[REDACTED]";

/// Redact secrets from text using gitleaks if available,
/// falling back to a basic pattern-based approach.
pub fn redact(text: &str) -> String {
    if let Some(redacted) = redact_with_gitleaks(text) {
        return redacted;
    }
    redact_basic(text)
}

/// Check if gitleaks is installed.
pub fn gitleaks_available() -> bool {
    Command::new("gitleaks")
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Use gitleaks to detect secrets in text content.
/// Returns redacted text or None if gitleaks is not available.
fn redact_with_gitleaks(text: &str) -> Option<String> {
    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join(format!("oobo-redact-{}.txt", std::process::id()));
    std::fs::write(&tmp_path, text).ok()?;

    let result = redact_with_gitleaks_inner(text, &tmp_path);
    let _ = std::fs::remove_file(&tmp_path);
    result
}

fn redact_with_gitleaks_inner(text: &str, tmp_path: &std::path::Path) -> Option<String> {
    let output = Command::new("gitleaks")
        .args([
            "detect",
            "--no-git",
            "--source",
            tmp_path.to_str()?,
            "--report-format",
            "json",
            "--report-path",
            "/dev/stdout",
            "--exit-code",
            "0",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let findings: Vec<GitleaksFinding> = serde_json::from_str(&stdout).ok()?;

    if findings.is_empty() {
        return Some(text.to_string());
    }

    let mut redacted = text.to_string();
    for finding in &findings {
        if !finding.secret.is_empty() {
            redacted = redacted.replace(&finding.secret, REDACT_PLACEHOLDER);
        }
    }

    Some(redacted)
}

/// Basic pattern-based redaction for common secret formats.
/// Used as fallback when gitleaks is not available.
fn redact_basic(text: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;

    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            r#"(?i)(sk|pk|api[_-]?key|token|secret|password|passwd|auth)[_\-]?\s*[:=]\s*['"]?([a-zA-Z0-9\-_.]{20,})['"]?"#,
            r"AKIA[0-9A-Z]{16}",
            r"(?i)(bearer|authorization)\s+[a-zA-Z0-9\-_.+/=]{30,}",
        ]
        .iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect()
    });

    let mut result = text.to_string();
    for pattern in patterns {
        result = pattern.replace_all(&result, REDACT_PLACEHOLDER).to_string();
    }
    result
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct GitleaksFinding {
    #[serde(default)]
    secret: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_basic_api_key() {
        let fake = format!("{}1234567890abcdefghij", "sk_live_");
        let text = format!("api_key = '{fake}'");
        let redacted = redact_basic(&text);
        assert!(redacted.contains(REDACT_PLACEHOLDER));
        assert!(!redacted.contains(&fake));
    }

    #[test]
    fn test_redact_basic_aws() {
        let fake = format!("{}{}", "AKIA", "IOSFODNN7EXAMPLE");
        let text = format!("aws_key = {fake}");
        let redacted = redact_basic(&text);
        assert!(redacted.contains(REDACT_PLACEHOLDER));
    }

    #[test]
    fn test_redact_basic_clean_text() {
        let text = "Hello world, this is a normal string.";
        let redacted = redact_basic(text);
        assert_eq!(redacted, text);
    }

    #[test]
    fn test_redact_basic_bearer_token() {
        let text = "Authorization Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9abcdefghijklmnop";
        let redacted = redact_basic(text);
        assert!(redacted.contains(REDACT_PLACEHOLDER));
        assert!(!redacted.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"));
    }

    #[test]
    fn test_redact_basic_password_field() {
        let text = "password = 'MySuperSecretPassword123456'";
        let redacted = redact_basic(text);
        assert!(redacted.contains(REDACT_PLACEHOLDER));
        assert!(!redacted.contains("MySuperSecretPassword123456"));
    }

    #[test]
    fn test_redact_basic_token_field() {
        let text = "token: abcdefghijklmnopqrstuvwxyz";
        let redacted = redact_basic(text);
        assert!(redacted.contains(REDACT_PLACEHOLDER));
    }

    #[test]
    fn test_redact_basic_multiple_secrets() {
        let sk = format!("{}abcdefghijklmnopqrst", "sk_test_");
        let ak = format!("{}{}", "AKIA", "IOSFODNN7EXAMPLE");
        let text = format!("api_key = '{sk}'\n{ak}\npassword = 'hunter2_really_long_password'");
        let redacted = redact_basic(&text);
        assert!(!redacted.contains(&sk));
        assert!(!redacted.contains(&ak));
        assert!(!redacted.contains("hunter2_really_long_password"));
    }

    #[test]
    fn test_redact_basic_short_values_not_redacted() {
        let text = "key = 'short'";
        let redacted = redact_basic(text);
        assert_eq!(redacted, text, "short values should not be redacted");
    }

    #[test]
    fn test_redact_basic_preserves_code() {
        let text = r#"fn main() {
    let x = 42;
    println!("Hello, world!");
}"#;
        let redacted = redact_basic(text);
        assert_eq!(redacted, text);
    }

    #[test]
    fn test_redact_calls_basic_as_fallback() {
        let text = "secret = 'this_is_a_very_long_secret_value_1234567890'";
        let result = redact(text);
        assert!(!result.contains("this_is_a_very_long_secret_value_1234567890"));
    }

    #[test]
    fn test_redact_placeholder_is_deterministic() {
        assert_eq!(REDACT_PLACEHOLDER, "[REDACTED]");
    }
}
