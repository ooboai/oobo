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

/// Full sanitization for any string that will be publicly visible
/// (orphan branch, remote payload, shared sessions).
/// Applies secret redaction first, then strips absolute paths.
pub fn sanitize_for_public(text: &str, project_root: &str) -> String {
    let redacted = redact(text);
    strip_absolute_paths(&redacted, project_root)
}

/// Replace absolute paths containing the project root with repo-relative paths.
/// Also strips the user's home directory from any remaining absolute paths.
pub fn strip_absolute_paths(text: &str, project_root: &str) -> String {
    let mut result = text.to_string();

    if !project_root.is_empty() {
        let root_slash = if project_root.ends_with('/') {
            project_root.to_string()
        } else {
            format!("{project_root}/")
        };
        result = result.replace(&root_slash, "");
    }

    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        let home_slash = format!("{home_str}/");
        result = result.replace(&home_slash, "~/");
    }

    result
}

/// Strip absolute paths from a file path string, returning a relative path.
/// Unlike `strip_absolute_paths` which does text replacement, this handles
/// a single path value — stripping the project root or replacing the home
/// directory prefix.
pub fn sanitize_path(path: &str, project_root: &str) -> String {
    if !path.starts_with('/') {
        return path.to_string();
    }

    if !project_root.is_empty() {
        let root_slash = if project_root.ends_with('/') {
            project_root.to_string()
        } else {
            format!("{project_root}/")
        };
        if path.starts_with(&root_slash) {
            return path[root_slash.len()..].to_string();
        }
        if path == project_root {
            return ".".to_string();
        }
    }

    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        let home_slash = format!("{home_str}/");
        if path.starts_with(home_slash.as_str()) {
            return format!("~/{}", &path[home_slash.len()..]);
        }
    }

    path.to_string()
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
    let tmp_path = tmp_dir.join(format!(
        "oobo-redact-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
    ));
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

    // ── basic pattern tests ──

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

    // ── JSONL transcript redaction tests ──

    #[test]
    fn test_redact_jsonl_transcript_with_api_key() {
        let sk = format!("{}abcdef1234567890abcdef", "sk_live_");
        let transcript = format!(
            r#"{{"role":"user","message":{{"content":"Set the API key to api_key = '{sk}'"}}}}"#
        );
        let redacted = redact(&transcript);
        assert!(
            !redacted.contains(&sk),
            "API key should be redacted from JSONL"
        );
        assert!(redacted.contains(REDACT_PLACEHOLDER));
        assert!(
            redacted.contains("role"),
            "JSON structure should be preserved"
        );
    }

    #[test]
    fn test_redact_jsonl_transcript_with_aws_creds() {
        let ak = format!("{}{}", "AKIA", "IOSFODNN7EXAMPLE");
        let transcript = format!(
            r#"{{"role":"assistant","message":{{"content":"Found credentials: {ak} in your config"}}}}"#
        );
        let redacted = redact(&transcript);
        assert!(
            !redacted.contains(&ak),
            "AWS key should be redacted from JSONL"
        );
    }

    #[test]
    fn test_redact_multiline_jsonl_transcript() {
        let secret = format!("{}abcdefghij1234567890", "sk_test_");
        let transcript = format!(
            "{}\n{}\n{}",
            r#"{{"role":"user","message":{{"content":"Please update the config"}}}}"#,
            format_args!(
                r#"{{"role":"assistant","message":{{"content":"I'll set token = '{secret}' in .env"}}}}"#
            ),
            r#"{"role":"user","message":{"content":"thanks that looks good"}}"#,
        );
        let redacted = redact(&transcript);
        assert!(
            !redacted.contains(&secret),
            "secret should be redacted across multi-line JSONL"
        );
        let line_count = redacted.lines().count();
        assert_eq!(line_count, 3, "line count should be preserved");
    }

    #[test]
    fn test_redact_jsonl_transcript_no_secrets() {
        let transcript = concat!(
            r#"{"role":"user","message":{"content":"refactor the parser module"}}"#,
            "\n",
            r#"{"role":"assistant","message":{"content":"I'll split it into lexer and parser"}}"#,
        );
        let redacted = redact(transcript);
        assert_eq!(
            redacted, transcript,
            "clean transcript should pass through unchanged"
        );
    }

    #[test]
    fn test_redact_jsonl_with_tool_calls() {
        let secret = format!("{}abcdefghij1234567890", "sk_live_");
        let transcript = format!(
            "{}\n{}\n{}",
            r#"{{"role":"user","message":{{"content":"deploy to prod"}}}}"#,
            format_args!(
                r#"{{"role":"assistant","message":{{"content":"Running deploy..."}},
                "tool_calls":[{{"name":"Shell","arguments":{{"command":"export API_KEY={secret} && deploy"}}}}]}}"#
            ),
            r#"{"role":"assistant","message":{"content":"Deploy complete."}}"#,
        );
        let redacted = redact(&transcript);
        assert!(
            !redacted.contains(&secret),
            "secret in tool call args should be redacted"
        );
    }

    #[test]
    fn test_redact_jsonl_with_password_in_env() {
        let transcript = r#"{"role":"assistant","message":{"content":"I'll update .env:\npassword = 'MyDatabasePassword1234567890'\nDB_HOST=localhost"}}"#;
        let redacted = redact(transcript);
        assert!(
            !redacted.contains("MyDatabasePassword1234567890"),
            "password in env content should be redacted"
        );
    }

    #[test]
    fn test_redact_jsonl_with_bearer_in_curl() {
        let transcript = r#"{"role":"assistant","message":{"content":"curl -H 'Authorization Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijk' https://api.example.com"}}"#;
        let redacted = redact(transcript);
        assert!(
            !redacted.contains("eyJhbGciOiJIUzI1NiJ9"),
            "bearer token in curl command should be redacted"
        );
    }

    #[test]
    fn test_redact_preserves_valid_json_structure() {
        let sk = format!("{}abcdefghij1234567890", "sk_test_");
        let line = format!(r#"{{"role":"user","message":{{"content":"secret = '{sk}'"}}}}"#);
        let redacted = redact(&line);
        assert!(
            serde_json::from_str::<serde_json::Value>(&redacted).is_ok(),
            "redacted JSONL line should still be valid JSON: {redacted}"
        );
    }

    // ── gitleaks integration test (skipped if gitleaks not installed) ──

    #[test]
    fn test_gitleaks_redacts_secrets_in_transcript() {
        if !gitleaks_available() {
            eprintln!("gitleaks not installed, skipping integration test");
            return;
        }

        let sk = format!("{}abcdefghij1234567890", "sk_live_");
        let transcript =
            format!(r#"{{"role":"user","message":{{"content":"Set api_key = '{sk}'"}}}}"#);
        let result = redact_with_gitleaks(&transcript);
        assert!(result.is_some(), "gitleaks should succeed");
        let redacted = result.unwrap();
        assert!(
            !redacted.contains(&sk),
            "gitleaks should redact the secret from transcript content"
        );
    }

    #[test]
    fn test_gitleaks_clean_transcript_passes_through() {
        if !gitleaks_available() {
            eprintln!("gitleaks not installed, skipping integration test");
            return;
        }

        let transcript = r#"{"role":"user","message":{"content":"refactor the parser"}}"#;
        let result = redact_with_gitleaks(transcript);
        assert!(result.is_some());
        assert_eq!(
            result.unwrap(),
            transcript,
            "clean content should pass through unchanged"
        );
    }

    #[test]
    fn test_gitleaks_multiline_transcript() {
        if !gitleaks_available() {
            eprintln!("gitleaks not installed, skipping integration test");
            return;
        }

        let aws_key = format!("{}{}", "AKIA", "IOSFODNN7EXAMPLE");
        let transcript = format!(
            "{}\n{}\n{}",
            r#"{{"role":"user","message":{{"content":"check my aws config"}}}}"#,
            format_args!(
                r#"{{"role":"assistant","message":{{"content":"Found key: {aws_key}"}}}}"#
            ),
            r#"{"role":"user","message":{"content":"please rotate that"}}"#,
        );
        let result = redact_with_gitleaks(&transcript);
        assert!(
            result.is_some(),
            "gitleaks should succeed on multi-line JSONL"
        );
        let redacted = result.unwrap();
        assert!(
            !redacted.contains(&aws_key),
            "gitleaks should redact AWS key"
        );
    }

    // ── strip_absolute_paths tests ──

    #[test]
    fn test_strip_absolute_paths_project_root() {
        let root = "/Users/teddy/dev/projects/trender";
        let input = "cd /Users/teddy/dev/projects/trender/src && cargo build";
        let result = strip_absolute_paths(input, root);
        assert_eq!(result, "cd src && cargo build");
    }

    #[test]
    fn test_strip_absolute_paths_home_fallback() {
        let root = "/Users/teddy/dev/projects/myapp";
        let home = dirs::home_dir().unwrap();
        let home_str = home.to_string_lossy();
        let input = format!("ls {home_str}/.config/something");
        let result = strip_absolute_paths(&input, root);
        assert_eq!(result, "ls ~/.config/something");
    }

    #[test]
    fn test_strip_absolute_paths_no_change_for_relative() {
        let root = "/Users/teddy/dev/projects/myapp";
        let input = "cargo test --release";
        let result = strip_absolute_paths(input, root);
        assert_eq!(result, "cargo test --release");
    }

    #[test]
    fn test_strip_absolute_paths_empty_root() {
        let input = "ls /some/path";
        let result = strip_absolute_paths(input, "");
        assert!(result.contains("/some/path"));
    }

    // ── sanitize_path tests ──

    #[test]
    fn test_sanitize_path_absolute_under_project() {
        let root = "/Users/teddy/dev/projects/myapp";
        let path = "/Users/teddy/dev/projects/myapp/src/lib.rs";
        assert_eq!(sanitize_path(path, root), "src/lib.rs");
    }

    #[test]
    fn test_sanitize_path_relative_passthrough() {
        let root = "/Users/teddy/dev/projects/myapp";
        let path = "src/lib.rs";
        assert_eq!(sanitize_path(path, root), "src/lib.rs");
    }

    #[test]
    fn test_sanitize_path_home_fallback() {
        let root = "/Users/teddy/dev/projects/myapp";
        let home = dirs::home_dir().unwrap();
        let home_str = home.to_string_lossy();
        let path = format!("{home_str}/.config/other.toml");
        let result = sanitize_path(&path, root);
        assert_eq!(result, "~/.config/other.toml");
    }

    #[test]
    fn test_sanitize_path_unrelated_absolute() {
        let root = "/Users/teddy/dev/projects/myapp";
        let path = "/etc/hosts";
        assert_eq!(sanitize_path(path, root), "/etc/hosts");
    }

    // ── sanitize_for_public tests ──

    #[test]
    fn test_sanitize_for_public_strips_paths_and_secrets() {
        let sk = format!("{}abcdefghij1234567890", "sk_live_");
        let root = "/Users/teddy/dev/projects/myapp";
        let input = format!("cd {root}/src && export TOKEN={sk}");
        let result = sanitize_for_public(&input, root);
        assert!(
            !result.contains("/Users/teddy"),
            "absolute path should be stripped"
        );
        assert!(!result.contains(&sk), "secret should be redacted");
        assert!(result.contains("cd src"), "relative path should remain");
    }

    #[test]
    fn test_sanitize_for_public_clean_text() {
        let root = "/Users/teddy/dev/projects/myapp";
        let input = "cargo build --release";
        let result = sanitize_for_public(input, root);
        assert_eq!(result, input);
    }
}
