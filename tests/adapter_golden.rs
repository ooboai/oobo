use std::path::PathBuf;

use oobo::core::message::Message;

#[test]
fn cursor_transcript_golden_stays_stable() {
    assert_messages_match(
        "cursor",
        "cursor-session.jsonl",
        "cursor-session.golden.json",
    );
}

#[test]
fn claude_transcript_golden_stays_stable() {
    assert_messages_match(
        "claude",
        "claude-session.jsonl",
        "claude-session.golden.json",
    );
}

#[test]
fn codex_transcript_golden_stays_stable() {
    assert_messages_match("codex", "codex-session.jsonl", "codex-session.golden.json");
}

#[test]
fn aider_transcript_golden_stays_stable() {
    assert_messages_match("aider", "aider-history.md", "aider-history.golden.json");
}

#[test]
fn copilot_transcript_golden_stays_stable() {
    assert_messages_match(
        "copilot",
        "copilot-session.json",
        "copilot-session.golden.json",
    );
}

#[test]
fn zed_transcript_golden_stays_stable() {
    assert_messages_match(
        "zed",
        "zed-conversation.json",
        "zed-conversation.golden.json",
    );
}

#[test]
fn gemini_transcript_golden_stays_stable() {
    assert_messages_match(
        "gemini",
        "gemini-session.json",
        "gemini-session.golden.json",
    );
}

fn assert_messages_match(tool: &str, fixture_name: &str, golden_name: &str) {
    let fixture = adapter_fixture(fixture_name);
    let actual = match tool {
        "cursor" => oobo::tools::cursor::transcript::parse_messages(&fixture),
        "claude" => oobo::tools::claude::transcript::parse_messages(&fixture),
        "codex" => oobo::tools::codex::transcript::parse_messages(&fixture),
        "aider" => oobo::tools::aider::transcript::parse_messages(&fixture),
        "copilot" => oobo::tools::copilot::transcript::parse_messages(&fixture),
        "zed" => oobo::tools::zed::transcript::parse_messages(&fixture),
        "gemini" => oobo::tools::gemini::transcript::parse_messages(&fixture),
        other => panic!("unknown adapter fixture tool: {other}"),
    };
    let actual = messages_to_json(&actual);
    let expected = std::fs::read_to_string(adapter_fixture(golden_name))
        .unwrap_or_else(|e| panic!("read golden fixture {golden_name}: {e}"));
    let expected: serde_json::Value = serde_json::from_str(&expected)
        .unwrap_or_else(|e| panic!("parse golden fixture {golden_name}: {e}"));

    assert_eq!(actual, expected, "{tool} transcript golden changed");
}

fn messages_to_json(messages: &[Message]) -> serde_json::Value {
    serde_json::to_value(messages).expect("messages serialize")
}

fn adapter_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("adapters")
        .join("v1")
        .join(name)
}
