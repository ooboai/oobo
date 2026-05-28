//! Integration tests for the oobo MCP server (stdio JSON-RPC).

use std::io::Write;
use std::process::{Command, Stdio};

fn oobo_binary() -> String {
    env!("CARGO_BIN_EXE_oobo").to_string()
}

fn mcp_session(requests: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let mut child = Command::new(oobo_binary())
        .args(["mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("OOBO_HOME", "/tmp/oobo-mcp-test")
        .spawn()
        .expect("Failed to start oobo mcp");

    let stdin = child.stdin.as_mut().unwrap();
    for req in requests {
        let line = serde_json::to_string(req).unwrap();
        writeln!(stdin, "{line}").unwrap();
    }
    drop(child.stdin.take());

    let output = child
        .wait_with_output()
        .expect("Failed to wait for process");
    assert!(output.status.success(), "oobo mcp exited with error");

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("Invalid JSON in MCP response"))
        .collect()
}

#[test]
fn mcp_initialize() {
    let responses = mcp_session(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    })]);

    assert_eq!(responses.len(), 1);
    let result = &responses[0]["result"];
    assert_eq!(result["protocolVersion"], "2024-11-05");
    assert_eq!(result["serverInfo"]["name"], "oobo");
    assert!(result["instructions"].as_str().unwrap().contains("search"));
}

#[test]
fn mcp_tools_list() {
    let responses = mcp_session(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    })]);

    assert_eq!(responses.len(), 1);
    let tools = responses[0]["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(
        names.contains(&"search"),
        "Should expose search tool (in git repo)"
    );
    assert!(
        names.contains(&"find_related"),
        "Should expose find_related tool"
    );
}

#[test]
fn mcp_search_returns_results() {
    let responses = mcp_session(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "search",
            "arguments": { "query": "configuration", "top_k": 2 }
        }
    })]);

    assert_eq!(responses.len(), 1);
    let result = &responses[0]["result"];
    assert!(result.get("isError").is_none(), "Should not be an error");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("results"), "Should contain search results");
}

#[test]
fn mcp_search_empty_query_errors() {
    let responses = mcp_session(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "search",
            "arguments": { "query": "" }
        }
    })]);

    assert_eq!(responses.len(), 1);
    let result = &responses[0]["result"];
    assert_eq!(result["isError"], true);
}

#[test]
fn mcp_recall_without_key_returns_error() {
    let responses = mcp_session(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "recall",
            "arguments": { "query": "auth migration" }
        }
    })]);

    assert_eq!(responses.len(), 1);
    let result = &responses[0]["result"];
    assert_eq!(result["isError"], true);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("OOBO_API_KEY"),
        "Should mention how to configure key"
    );
}

#[test]
fn mcp_unknown_tool_returns_error() {
    let responses = mcp_session(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tools/call",
        "params": {
            "name": "nonexistent_tool",
            "arguments": {}
        }
    })]);

    assert_eq!(responses.len(), 1);
    let result = &responses[0]["result"];
    assert_eq!(result["isError"], true);
}

#[test]
fn mcp_ping() {
    let responses = mcp_session(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "ping",
        "params": {}
    })]);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["result"], serde_json::json!({}));
}

#[test]
fn mcp_invalid_json_returns_parse_error() {
    let mut child = Command::new(oobo_binary())
        .args(["mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("OOBO_HOME", "/tmp/oobo-mcp-test")
        .spawn()
        .expect("Failed to start oobo mcp");

    let stdin = child.stdin.as_mut().unwrap();
    writeln!(stdin, "not valid json {{{{").unwrap();
    drop(child.stdin.take());

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let resp: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(resp["error"]["code"].as_i64().unwrap() == -32700);
}

#[test]
fn mcp_notification_returns_nothing() {
    let mut child = Command::new(oobo_binary())
        .args(["mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("OOBO_HOME", "/tmp/oobo-mcp-test")
        .spawn()
        .expect("Failed to start oobo mcp");

    let stdin = child.stdin.as_mut().unwrap();
    // Notification (no id) should produce no response
    let notification = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    });
    writeln!(stdin, "{}", serde_json::to_string(&notification).unwrap()).unwrap();
    drop(child.stdin.take());

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "Notifications should produce no output"
    );
}

#[test]
fn mcp_multi_request_session() {
    let responses = mcp_session(&[
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "ping", "params": {}}),
    ]);

    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(responses[1]["id"], 2);
    assert_eq!(responses[2]["id"], 3);
}

#[test]
fn mcp_get_context_without_key_returns_error() {
    let responses = mcp_session(&[serde_json::json!({
        "jsonrpc": "2.0",
        "id": 8,
        "method": "tools/call",
        "params": {
            "name": "get_context",
            "arguments": { "topic": "auth flow", "budget_tokens": 2000 }
        }
    })]);

    assert_eq!(responses.len(), 1);
    let result = &responses[0]["result"];
    assert_eq!(result["isError"], true);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("OOBO_API_KEY"));
}

#[test]
fn mcp_tools_list_includes_get_context_with_key() {
    let mut child = std::process::Command::new(oobo_binary())
        .args(["mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("OOBO_HOME", "/tmp/oobo-mcp-test")
        .env("OOBO_API_KEY", "sk-oobo-v1-testkey123456789012345678901234")
        .spawn()
        .expect("Failed to start oobo mcp");

    let stdin = child.stdin.as_mut().unwrap();
    let req = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}});
    writeln!(stdin, "{}", serde_json::to_string(&req).unwrap()).unwrap();
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let resp: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let tools = resp["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(
        names.contains(&"get_context"),
        "get_context should appear when key is set"
    );
    assert!(names.contains(&"recall"));
    assert!(names.contains(&"ask"));
}
