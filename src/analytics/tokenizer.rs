#![allow(dead_code)]

use tiktoken_rs::{cl100k_base, o200k_base, CoreBPE};

/// Model families for selecting the appropriate tokenizer encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFamily {
    /// Claude models, GPT-4, GPT-3.5 — uses cl100k_base
    Cl100k,
    /// GPT-4o, o1, o3 family — uses o200k_base
    O200k,
}

/// Detect the appropriate model family from a model name string.
pub fn detect_family(model: &str) -> ModelFamily {
    let m = model.to_lowercase();
    if m.contains("o200k")
        || m.contains("gpt-4o")
        || m.contains("gpt-5")
        || m.starts_with("o1")
        || m.starts_with("o3")
        || m.starts_with("o4")
        || m.contains("-o1")
        || m.contains("-o3")
        || m.contains("-o4")
    {
        return ModelFamily::O200k;
    }
    // Default to cl100k — works well for Claude and GPT-4
    ModelFamily::Cl100k
}

fn get_bpe(family: ModelFamily) -> Option<CoreBPE> {
    match family {
        ModelFamily::Cl100k => cl100k_base().ok(),
        ModelFamily::O200k => o200k_base().ok(),
    }
}

/// Count the number of tokens in `text` using the given model family's encoding.
pub fn count_tokens(text: &str, family: ModelFamily) -> u64 {
    if text.is_empty() {
        return 0;
    }
    match get_bpe(family) {
        Some(bpe) => bpe.encode_with_special_tokens(text).len() as u64,
        None => (text.len() as u64) / 4,
    }
}

/// Count tokens using the default encoding (cl100k_base).
/// Suitable as a general-purpose fallback when the model is unknown.
pub fn count_tokens_default(text: &str) -> u64 {
    count_tokens(text, ModelFamily::Cl100k)
}

/// Sum token counts for user messages in a conversation.
pub fn count_input_tokens(messages: &[(String, String)], family: ModelFamily) -> u64 {
    messages
        .iter()
        .filter(|(role, _)| role == "user")
        .map(|(_, text)| count_tokens(text, family))
        .sum()
}

/// Sum token counts for assistant messages in a conversation.
pub fn count_output_tokens(messages: &[(String, String)], family: ModelFamily) -> u64 {
    messages
        .iter()
        .filter(|(role, _)| role == "assistant")
        .map(|(_, text)| count_tokens(text, family))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_family_claude() {
        assert_eq!(
            detect_family("claude-opus-4-5-20251101"),
            ModelFamily::Cl100k
        );
        assert_eq!(
            detect_family("claude-sonnet-4-20251001"),
            ModelFamily::Cl100k
        );
    }

    #[test]
    fn test_detect_family_gpt4o() {
        assert_eq!(detect_family("gpt-4o-mini"), ModelFamily::O200k);
        assert_eq!(detect_family("gpt-4o"), ModelFamily::O200k);
    }

    #[test]
    fn test_detect_family_o1() {
        assert_eq!(detect_family("o1-preview"), ModelFamily::O200k);
        assert_eq!(detect_family("o3-mini"), ModelFamily::O200k);
    }

    #[test]
    fn test_detect_family_gpt4() {
        assert_eq!(detect_family("gpt-4"), ModelFamily::Cl100k);
        assert_eq!(detect_family("gpt-4-turbo"), ModelFamily::Cl100k);
    }

    #[test]
    fn test_detect_family_unknown() {
        assert_eq!(detect_family("some-unknown-model"), ModelFamily::Cl100k);
    }

    #[test]
    fn test_detect_family_no_false_positives() {
        assert_eq!(detect_family("proto1-model"), ModelFamily::Cl100k);
        assert_eq!(detect_family("co1umn-reader"), ModelFamily::Cl100k);
        assert_eq!(
            detect_family("anthropic/claude-sonnet-4-20250514"),
            ModelFamily::Cl100k
        );
    }

    #[test]
    fn test_count_tokens_empty() {
        assert_eq!(count_tokens("", ModelFamily::Cl100k), 0);
    }

    #[test]
    fn test_count_tokens_basic() {
        let count = count_tokens("Hello, world!", ModelFamily::Cl100k);
        assert!(count > 0);
        assert!(count < 10);
    }

    #[test]
    fn test_count_tokens_default() {
        let count = count_tokens_default("The quick brown fox jumps over the lazy dog.");
        assert!(count > 0);
    }

    #[test]
    fn test_count_input_output_tokens() {
        let messages = vec![
            ("user".to_string(), "What is Rust?".to_string()),
            (
                "assistant".to_string(),
                "Rust is a multi-paradigm, general-purpose programming language that emphasizes performance, type safety, and concurrency. It achieves memory safety without garbage collection through its ownership and borrowing system.".to_string(),
            ),
            ("user".to_string(), "Tell me more.".to_string()),
        ];

        let input = count_input_tokens(&messages, ModelFamily::Cl100k);
        let output = count_output_tokens(&messages, ModelFamily::Cl100k);

        assert!(input > 0);
        assert!(output > 0);
        assert!(output > input);
    }
}
