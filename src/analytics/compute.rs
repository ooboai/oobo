use super::tokenizer;
use crate::core::message::Message;
use crate::db::stats::StatsRow;

/// Native telemetry extracted directly from tool-specific data.
#[derive(Debug, Clone, Default)]
pub struct NativeStats {
    pub model: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub duration_secs: Option<u64>,
    pub files_touched: Vec<String>,
    pub tool_call_count: u32,
}

/// Build a StatsRow by combining native telemetry (when available) with
/// tiktoken estimation (as fallback) and pricing lookup.
pub fn compute_session_stats(
    session_id: &str,
    source: &str,
    messages: &[Message],
    native: Option<NativeStats>,
) -> StatsRow {
    let now = chrono::Utc::now().timestamp();
    let native = native.unwrap_or_default();
    let model = native.model.clone();

    let family = model
        .as_deref()
        .map(tokenizer::detect_family)
        .unwrap_or(tokenizer::ModelFamily::Cl100k);

    let pairs: Vec<(String, String)> = messages
        .iter()
        .map(|m| (m.role.clone(), m.text.clone()))
        .collect();

    let (input_tokens, output_tokens, token_source) =
        if native.input_tokens.is_some() || native.output_tokens.is_some() {
            (
                native.input_tokens.unwrap_or(0),
                native.output_tokens.unwrap_or(0),
                "native",
            )
        } else if !messages.is_empty() {
            let inp = tokenizer::count_input_tokens(&pairs, family);
            let out = tokenizer::count_output_tokens(&pairs, family);
            (inp, out, "tiktoken")
        } else {
            (0, 0, "unknown")
        };

    let is_estimated = token_source != "native";

    let cache_read = native.cache_read_tokens.unwrap_or(0);
    let cache_creation = native.cache_creation_tokens.unwrap_or(0);

    let duration = native.duration_secs.or_else(|| compute_duration(messages));

    StatsRow {
        session_id: session_id.to_string(),
        source: source.to_string(),
        model,
        input_tokens: Some(input_tokens as i64),
        output_tokens: Some(output_tokens as i64),
        cache_read_tokens: Some(cache_read as i64),
        cache_creation_tokens: Some(cache_creation as i64),
        is_estimated,
        token_source: token_source.to_string(),
        duration_secs: duration.map(|d| d as i64),
        files_touched: native.files_touched,
        tool_call_count: native.tool_call_count as i32,
        computed_at: now,
    }
}

/// Estimate session duration from message timestamps (first to last).
fn compute_duration(messages: &[Message]) -> Option<u64> {
    if messages.len() < 2 {
        return None;
    }
    let first_ts = messages.first()?.timestamp_ms?;
    let last_ts = messages.last()?.timestamp_ms?;
    if last_ts > first_ts {
        Some(((last_ts - first_ts) / 1000) as u64)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_messages() -> Vec<Message> {
        vec![
            Message {
                role: "user".into(),
                text: "What is Rust programming language?".into(),
                timestamp_ms: Some(1000000),
            },
            Message {
                role: "assistant".into(),
                text: "Rust is a multi-paradigm, general-purpose programming language that emphasizes performance, type safety, and concurrency.".into(),
                timestamp_ms: Some(1060000),
            },
        ]
    }

    #[test]
    fn test_compute_with_native_stats() {
        let native = NativeStats {
            model: Some("claude-sonnet-4".into()),
            input_tokens: Some(5000),
            output_tokens: Some(3000),
            cache_read_tokens: Some(1000),
            cache_creation_tokens: Some(200),
            duration_secs: Some(45),
            files_touched: vec!["src/main.rs".into()],
            tool_call_count: 2,
        };

        let stats = compute_session_stats("s1", "claude", &[], Some(native));
        assert_eq!(stats.input_tokens, Some(5000));
        assert_eq!(stats.output_tokens, Some(3000));
        assert!(!stats.is_estimated);
        assert_eq!(stats.token_source, "native");
        assert_eq!(stats.duration_secs, Some(45));
    }

    #[test]
    fn test_compute_with_tiktoken_fallback() {
        let messages = make_messages();
        let stats = compute_session_stats("s2", "cursor", &messages, None);
        assert!(stats.input_tokens.unwrap_or(0) > 0);
        assert!(stats.output_tokens.unwrap_or(0) > 0);
        assert!(stats.is_estimated);
        assert_eq!(stats.token_source, "tiktoken");
    }

    #[test]
    fn test_compute_duration_from_messages() {
        let messages = make_messages();
        let dur = compute_duration(&messages);
        assert_eq!(dur, Some(60)); // 60000ms / 1000 = 60s
    }

    #[test]
    fn test_compute_no_messages() {
        let stats = compute_session_stats("s3", "cursor", &[], None);
        assert_eq!(stats.input_tokens, Some(0));
        assert_eq!(stats.output_tokens, Some(0));
        assert_eq!(stats.token_source, "unknown");
    }

}
