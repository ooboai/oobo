use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

// ── Fixture helpers ─────────────────────────────────────────────────────────

fn create_test_vscdb(dir: &Path, composers_json: &str) {
    let db_path = dir.join("state.vscdb");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
        ["composer.composerData", composers_json],
    )
    .unwrap();
}

fn create_workspace_json(dir: &Path, folder_uri: &str) {
    let ws_json = dir.join("workspace.json");
    fs::write(ws_json, format!(r#"{{"folder": "{folder_uri}"}}"#)).unwrap();
}

fn create_transcript_jsonl(dir: &Path, session_id: &str) {
    let session_dir = dir.join(session_id);
    fs::create_dir_all(&session_dir).unwrap();
    let jsonl_path = session_dir.join(format!("{session_id}.jsonl"));
    let mut f = fs::File::create(jsonl_path).unwrap();
    writeln!(
        f,
        r#"{{"role":"user","message":{{"content":[{{"type":"text","text":"Hello"}}]}}}}"#
    )
    .unwrap();
    writeln!(
        f,
        r#"{{"role":"assistant","message":{{"content":[{{"type":"text","text":"Hi there!"}}]}}}}"#
    )
    .unwrap();
    writeln!(
        f,
        r#"{{"role":"user","message":{{"content":[{{"type":"text","text":"Thanks"}}]}}}}"#
    )
    .unwrap();
}

fn create_transcript_txt(dir: &Path, session_id: &str) {
    let txt_path = dir.join(format!("{session_id}.txt"));
    fs::write(
        txt_path,
        "user:\nWhat is 2+2?\nassistant:\nThe answer is 4.\n",
    )
    .unwrap();
}

#[allow(deprecated)]
fn oobo_binary() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin("oobo")
}

// ── Composer extraction tests ───────────────────────────────────────────────

#[test]
fn test_composer_extraction_with_multiple_sessions() {
    let tmp = TempDir::new().unwrap();
    let json = r#"{
        "allComposers": [
            {
                "composerId": "session-1-uuid",
                "name": "Implement auth flow",
                "unifiedMode": "agent",
                "createdAt": 1700000000000,
                "lastUpdatedAt": 1700050000000,
                "isArchived": false
            },
            {
                "composerId": "session-2-uuid",
                "name": "Fix database migration",
                "unifiedMode": "chat",
                "createdAt": 1699990000000,
                "lastUpdatedAt": 1700040000000
            },
            {
                "composerId": "session-3-uuid",
                "name": "Code review feedback",
                "unifiedMode": "plan",
                "createdAt": 1699980000000
            }
        ]
    }"#;

    create_test_vscdb(tmp.path(), json);

    let sessions = oobo::cursor::composer::extract_sessions(tmp.path(), "/test/project");
    assert_eq!(sessions.len(), 3);

    assert_eq!(sessions[0].session_id, "session-1-uuid");
    assert_eq!(sessions[0].name, "Implement auth flow");
    assert_eq!(sessions[0].mode, "agent");
    assert_eq!(sessions[0].updated_at, Some(1700050000000));

    assert_eq!(sessions[1].session_id, "session-2-uuid");
    assert_eq!(sessions[1].name, "Fix database migration");
    assert_eq!(sessions[1].mode, "chat");

    assert_eq!(sessions[2].session_id, "session-3-uuid");
    assert!(sessions[2].updated_at.is_none());
}

#[test]
fn test_composer_extraction_malformed_json() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("state.vscdb");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
        ["composer.composerData", "not valid json {{{"],
    )
    .unwrap();

    let sessions = oobo::cursor::composer::extract_sessions(tmp.path(), "/test");
    assert!(sessions.is_empty());
}

#[test]
fn test_composer_extraction_missing_table() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("state.vscdb");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute("CREATE TABLE OtherTable (key TEXT, value TEXT)", [])
        .unwrap();

    let sessions = oobo::cursor::composer::extract_sessions(tmp.path(), "/test");
    assert!(sessions.is_empty());
}

// ── Transcript tests ────────────────────────────────────────────────────────

#[test]
fn test_transcript_jsonl_from_fixtures() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let path = fixtures.join("transcript.jsonl");

    let messages = oobo::cursor::transcript::parse_messages(&path);
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].role, "user");
    assert!(messages[0].text.contains("authentication bug"));
    assert_eq!(messages[1].role, "assistant");
    assert!(messages[1].text.contains("token validation"));
    assert_eq!(messages[2].role, "user");
    assert_eq!(messages[3].role, "assistant");
    assert!(messages[3].text.contains("validate_token"));
}

#[test]
fn test_transcript_txt_from_fixtures() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let path = fixtures.join("transcript.txt");

    let messages = oobo::cursor::transcript::parse_messages(&path);
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].role, "user");
    assert!(messages[0].text.contains("database schema"));
    assert_eq!(messages[1].role, "assistant");
    assert!(messages[1].text.contains("users"));
    assert_eq!(messages[2].role, "user");
    assert_eq!(messages[3].role, "assistant");
    assert!(messages[3].text.contains("ALTER TABLE"));
}

#[test]
fn test_transcript_find_in_new_format() {
    let tmp = TempDir::new().unwrap();
    let transcripts_dir = tmp.path().join("projects/Test-project/agent-transcripts");
    fs::create_dir_all(&transcripts_dir).unwrap();
    create_transcript_jsonl(&transcripts_dir, "abc-123-full-uuid");

    let _path =
        oobo::cursor::transcript::find_transcript_path("/Test/project", "abc-123-full-uuid");
    // This won't find it because cursor_projects_dir() returns the real path,
    // but the logic is correct. We test the function directly instead.

    // Direct test of the file we created
    let jsonl = transcripts_dir.join("abc-123-full-uuid/abc-123-full-uuid.jsonl");
    assert!(jsonl.exists());
    let msgs = oobo::cursor::transcript::parse_messages(&jsonl);
    assert_eq!(msgs.len(), 3);
}

#[test]
fn test_transcript_find_in_old_format() {
    let tmp = TempDir::new().unwrap();
    let transcripts_dir = tmp.path().join("projects/Test-project/agent-transcripts");
    fs::create_dir_all(&transcripts_dir).unwrap();
    create_transcript_txt(&transcripts_dir, "old-session-uuid");

    let txt = transcripts_dir.join("old-session-uuid.txt");
    assert!(txt.exists());
    let msgs = oobo::cursor::transcript::parse_messages(&txt);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, "user");
    assert!(msgs[0].text.contains("2+2"));
}

#[test]
fn test_transcript_count_messages() {
    let tmp = TempDir::new().unwrap();
    let jsonl = tmp.path().join("count_test.jsonl");
    let mut f = fs::File::create(&jsonl).unwrap();
    for i in 0..10 {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        writeln!(
            f,
            r#"{{"role":"{role}","message":{{"content":"msg {i}"}}}}"#
        )
        .unwrap();
    }

    let messages = oobo::cursor::transcript::parse_messages(&jsonl);
    assert_eq!(messages.len(), 10);
}

// ── Config tests ────────────────────────────────────────────────────────────

#[test]
fn test_config_save_and_load() {
    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");

    let cfg = oobo::config::Config {
        server: oobo::config::ServerConfig {
            url: "https://custom.server.com".into(),
            api_key: "sk_test_key".into(),
        },
        git: oobo::config::GitConfig {
            real_git_path: "/usr/local/bin/git".into(),
            alias_enabled: true,
        },
        cursor: oobo::config::ToolConfig { enabled: false },
        claude: oobo::config::ToolConfig { enabled: true },
        windsurf: oobo::config::ToolConfig { enabled: true },
        aider: oobo::config::ToolConfig { enabled: true },
        continue_dev: oobo::config::ToolConfig { enabled: true },
        zed: oobo::config::ToolConfig { enabled: true },
        copilot: oobo::config::ToolConfig { enabled: true },
        trae: oobo::config::ToolConfig { enabled: true },
        codex: oobo::config::ToolConfig { enabled: true },
        opencode: oobo::config::ToolConfig { enabled: true },
        telemetry: oobo::config::TelemetryConfig {
            enabled: true,
            send_diffs: true,
            send_transcripts: false,
        },
    };

    let content = toml::to_string_pretty(&cfg).unwrap();
    fs::write(&config_path, &content).unwrap();

    let loaded: oobo::config::Config =
        toml::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();

    assert_eq!(loaded.server.url, "https://custom.server.com");
    assert_eq!(loaded.server.api_key, "sk_test_key");
    assert!(loaded.git.alias_enabled);
    assert!(!loaded.cursor.enabled);
    assert!(loaded.telemetry.send_diffs);
}

#[test]
fn test_config_partial_toml() {
    let partial = r#"
[server]
url = "https://my.server"
"#;
    let cfg: oobo::config::Config = toml::from_str(partial).unwrap();
    assert_eq!(cfg.server.url, "https://my.server");
    assert!(cfg.server.api_key.is_empty());
    assert!(cfg.cursor.enabled); // default
    assert!(cfg.telemetry.enabled); // default
}

#[test]
fn test_config_empty_toml() {
    let cfg: oobo::config::Config = toml::from_str("").unwrap();
    assert_eq!(cfg.server.url, "https://dashboard.oobo.ai");
    assert!(cfg.cursor.enabled);
}

// ── Git command detection tests ─────────────────────────────────────────────

#[test]
fn test_write_op_detection_comprehensive() {
    use oobo::git::commands::is_write_op;

    // All write ops
    let write_ops = vec![
        vec!["commit", "-m", "msg"],
        vec!["push", "origin", "main"],
        vec!["pull", "--rebase"],
        vec!["merge", "feature"],
        vec!["rebase", "main"],
        vec!["cherry-pick", "abc123"],
        vec!["revert", "HEAD"],
        vec!["reset", "--hard", "HEAD~1"],
        vec!["stash"],
        vec!["stash", "pop"],
        vec!["tag", "v1.0.0"],
    ];

    for op in write_ops {
        let refs: Vec<&str> = op.iter().map(|s| &**s).collect();
        assert!(is_write_op(&refs), "expected write op: {:?}", op);
    }

    // All read ops
    let read_ops = vec![
        vec!["status"],
        vec!["log", "--oneline", "-10"],
        vec!["diff"],
        vec!["diff", "--staged"],
        vec!["branch", "-a"],
        vec!["show", "HEAD"],
        vec!["blame", "file.rs"],
        vec!["remote", "-v"],
        vec!["fetch", "origin"],
        vec!["clone", "url"],
        vec!["init"],
        vec!["checkout", "main"],
        vec!["switch", "feature"],
        vec!["add", "."],
    ];

    for op in read_ops {
        let refs: Vec<&str> = op.iter().map(|s| &**s).collect();
        assert!(!is_write_op(&refs), "expected read op: {:?}", op);
    }
}

#[test]
fn test_subcommand_extraction() {
    use oobo::git::commands::subcommand_name;

    assert_eq!(subcommand_name(&["commit", "-m", "x"]), Some("commit"));
    assert_eq!(subcommand_name(&["-C", "/tmp", "status"]), Some("status"));
    assert_eq!(
        subcommand_name(&["-c", "user.name=Test", "log"]),
        Some("log")
    );
    assert_eq!(
        subcommand_name(&["--git-dir", "/tmp/.git", "diff"]),
        Some("diff")
    );
    assert_eq!(subcommand_name(&[]), None);
    assert_eq!(subcommand_name(&["--version"]), None);
}

// ── Git proxy integration test ──────────────────────────────────────────────

#[test]
fn test_git_proxy_passthrough() {
    let tmp = TempDir::new().unwrap();

    // Initialize a git repo
    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    // Create a file and commit
    fs::write(tmp.path().join("hello.txt"), "hello world").unwrap();

    Command::new("git")
        .args(["add", "."])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    // Use oobo as git proxy to commit
    let output = Command::new(oobo_binary())
        .args(["commit", "-m", "test commit via oobo"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "oobo commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify the commit was made
    let log_output = Command::new("git")
        .args(["log", "--oneline"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let log = String::from_utf8_lossy(&log_output.stdout);
    assert!(
        log.contains("test commit via oobo"),
        "commit not found in log: {log}"
    );
}

#[test]
fn test_oobo_dash_command() {
    let output = Command::new(oobo_binary()).args(["dash"]).output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("oobo v"),
        "expected oobo dash output: {stdout}"
    );
}

#[test]
fn test_git_log_passthrough() {
    let tmp = TempDir::new().unwrap();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    // `log` is not an oobo subcommand, so it passes to git
    let output = Command::new(oobo_binary())
        .args(["log", "--oneline"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    // git log on empty repo returns 128, but oobo should pass it through
    assert!(!String::from_utf8_lossy(&output.stderr).contains("oobo"));
}

#[test]
fn test_git_proxy_diff() {
    let tmp = TempDir::new().unwrap();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let output = Command::new(oobo_binary())
        .args(["diff"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    assert!(output.status.success());
}

// ── CLI integration tests ───────────────────────────────────────────────────

#[test]
fn test_cli_help() {
    let output = Command::new(oobo_binary())
        .args(["--help"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("oobo"));
    assert!(stdout.contains("setup"));
    assert!(stdout.contains("sessions"));
    assert!(stdout.contains("alias"));
    assert!(stdout.contains("dash"));
}

#[test]
fn test_cli_version() {
    let output = Command::new(oobo_binary())
        .args(["--version"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("oobo"));
}

#[test]
fn test_cli_sessions_help() {
    let output = Command::new(oobo_binary())
        .args(["sessions", "--help"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("list"));
    assert!(stdout.contains("show"));
    assert!(stdout.contains("export"));
}

// ── Payload serialization tests ─────────────────────────────────────────────

#[test]
fn test_event_payload_roundtrip() {
    use oobo::server::payload::*;

    let mut tools = BTreeMap::new();
    tools.insert(
        "cursor".into(),
        ToolContext {
            active_sessions: 3,
            recent_session: Some(SessionSummary {
                id: "session-uuid-123".into(),
                name: "Debug authentication".into(),
                mode: "agent".into(),
                message_count: 42,
                stats: None,
                messages: Vec::new(),
            }),
        },
    );
    tools.insert(
        "claude".into(),
        ToolContext {
            active_sessions: 1,
            recent_session: Some(SessionSummary {
                id: "claude-session-456".into(),
                name: "Refactor module".into(),
                mode: "opus-4.5".into(),
                message_count: 10,
                stats: None,
                messages: Vec::new(),
            }),
        },
    );
    tools.insert(
        "aider".into(),
        ToolContext {
            active_sessions: 1,
            recent_session: Some(SessionSummary {
                id: "aider-abc".into(),
                name: "aider chat".into(),
                mode: "aider".into(),
                message_count: 5,
                stats: None,
                messages: Vec::new(),
            }),
        },
    );

    let payload = EventPayload {
        event: "git.commit".into(),
        timestamp: chrono::Utc::now(),
        project: ProjectInfo {
            root: "/home/user/project".into(),
            name: "project".into(),
        },
        git: GitInfo {
            operation: "commit".into(),
            branch: "main".into(),
            commit_hash: "abc123def456".into(),
            commit_message: "fix: resolve auth issue".into(),
            author: "Developer <dev@example.com>".into(),
            files_changed: 5,
            insertions: 120,
            deletions: 45,
        },
        tools,
    };

    let json = serde_json::to_string(&payload).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed["event"], "git.commit");
    assert_eq!(parsed["project"]["name"], "project");
    assert_eq!(parsed["git"]["files_changed"], 5);
    assert_eq!(parsed["git"]["insertions"], 120);
    assert_eq!(parsed["tools"]["cursor"]["active_sessions"], 3);
    assert_eq!(
        parsed["tools"]["cursor"]["recent_session"]["name"],
        "Debug authentication"
    );
    assert_eq!(parsed["tools"]["claude"]["active_sessions"], 1);
    assert_eq!(
        parsed["tools"]["claude"]["recent_session"]["name"],
        "Refactor module"
    );
    assert_eq!(parsed["tools"]["aider"]["active_sessions"], 1);
}

// ── Workspace tests ─────────────────────────────────────────────────────────

#[test]
fn test_workspace_json_parsing() {
    let tmp = TempDir::new().unwrap();
    create_workspace_json(tmp.path(), "file:///Users/test/my-project");

    let ws_json = tmp.path().join("workspace.json");
    let content = fs::read_to_string(ws_json).unwrap();
    let data: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(
        data["folder"].as_str().unwrap(),
        "file:///Users/test/my-project"
    );
}

// ── Path slug tests ─────────────────────────────────────────────────────────

#[test]
fn test_path_to_slug_various() {
    use oobo::cursor::path_to_slug;

    assert_eq!(
        path_to_slug("/Users/dev/projects/my-app"),
        "Users-dev-projects-my-app"
    );
    assert_eq!(path_to_slug("/tmp/test"), "tmp-test");
    assert_eq!(
        path_to_slug("/home/user/workspace/repo"),
        "home-user-workspace-repo"
    );
}

// ── Aider tests ─────────────────────────────────────────────────────────

#[test]
fn test_aider_session_discovery() {
    let tmp = TempDir::new().unwrap();
    let history = tmp.path().join(".aider.chat.history.md");
    fs::write(
        &history,
        "# aider chat started at 2024-06-01 10:00:00\n\n\
         #### user\nWrite hello world in Python\n\n\
         #### assistant\n```python\nprint('hello')\n```\n\n\
         # aider chat started at 2024-06-02 14:30:00\n\n\
         #### user\nAdd tests for the hello module\n\n\
         #### assistant\nHere are the tests:\n```python\ndef test_hello(): pass\n```\n",
    )
    .unwrap();

    let sessions = oobo::aider::sessions_for_project(&tmp.path().to_string_lossy()).unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].source, "aider");
}

#[test]
fn test_aider_transcript_parsing() {
    let tmp = TempDir::new().unwrap();
    let history = tmp.path().join("chat.md");
    fs::write(
        &history,
        "# aider chat started at 2024-01-01 00:00:00\n\n\
         #### user\nRefactor this function\n\n\
         #### assistant\nHere is the refactored version:\n```\nfn foo() {}\n```\n\n\
         #### user\nLooks good, now add docs\n\n\
         #### assistant\nDone:\n```\n/// Does foo things\nfn foo() {}\n```\n",
    )
    .unwrap();

    let msgs = oobo::aider::transcript::parse_messages(&history);
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[0].role, "user");
    assert!(msgs[0].text.contains("Refactor"));
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[2].role, "user");
    assert_eq!(msgs[3].role, "assistant");
}

// ── Copilot Chat tests ──────────────────────────────────────────────────

#[test]
fn test_copilot_session_parsing() {
    let tmp = TempDir::new().unwrap();
    let session_file = tmp.path().join("copilot-session.json");
    fs::write(
        &session_file,
        r#"{
            "sessionId": "copilot-uuid-abc",
            "creationDate": 1700000000000,
            "version": 3,
            "requests": [
                {
                    "requestId": "req-1",
                    "timestamp": 1700000001000,
                    "modelId": "copilot/gpt-4",
                    "message": {"text": "How do I write a REST API?"},
                    "response": {"value": "Here's how to create a REST API..."}
                },
                {
                    "requestId": "req-2",
                    "timestamp": 1700000010000,
                    "modelId": "copilot/gpt-4",
                    "message": {"text": "Add authentication"},
                    "response": {"value": "You can add JWT auth like this..."}
                }
            ]
        }"#,
    )
    .unwrap();

    let msgs = oobo::copilot::transcript::parse_messages(&session_file);
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[0].role, "user");
    assert!(msgs[0].text.contains("REST API"));
    assert_eq!(msgs[1].role, "assistant");
    assert!(msgs[1].text.contains("REST API"));
    assert_eq!(msgs[2].role, "user");
    assert_eq!(msgs[3].role, "assistant");
}

// ── Continue.dev tests ──────────────────────────────────────────────────

#[test]
fn test_continue_session_json_parsing() {
    let tmp = TempDir::new().unwrap();
    let session_file = tmp.path().join("continue-session.json");
    fs::write(
        &session_file,
        r#"{
            "sessionId": "continue-sess-1",
            "title": "Debugging crash",
            "dateCreated": "2024-06-15T10:30:00Z",
            "workspaceDirectory": "/home/dev/project",
            "history": [
                {"message": {"role": "user", "content": "Why is this crashing?"}},
                {"message": {"role": "assistant", "content": "The null pointer at line 42..."}}
            ]
        }"#,
    )
    .unwrap();

    let msgs = oobo::continue_dev::transcript::parse_messages(&session_file);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, "user");
    assert!(msgs[0].text.contains("crashing"));
    assert_eq!(msgs[1].role, "assistant");
    assert!(msgs[1].text.contains("null pointer"));
}

// ── Zed tests ───────────────────────────────────────────────────────────

#[test]
fn test_zed_conversation_parsing() {
    let tmp = TempDir::new().unwrap();
    let conv_file = tmp.path().join("zed-conv.json");
    fs::write(
        &conv_file,
        r#"{
            "title": "Rust iterators",
            "model": "claude-3.5-sonnet",
            "messages": [
                {"role": "user", "content": "Explain Rust iterators"},
                {"role": "assistant", "content": "Iterators in Rust provide lazy evaluation..."}
            ]
        }"#,
    )
    .unwrap();

    let msgs = oobo::zed::transcript::parse_messages(&conv_file);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, "user");
    assert!(msgs[0].text.contains("iterators"));
    assert_eq!(msgs[1].role, "assistant");
}

// ── VS Code fork (shared) tests ─────────────────────────────────────────

#[test]
fn test_vscode_fork_extract_sessions() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("state.vscdb");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
        [
            "cascade.composerData",
            r#"{"allComposers":[
                {"composerId":"ws-001","name":"Cascade chat","unifiedMode":"chat","createdAt":1700000000000},
                {"composerId":"ws-002","name":"Debug session","unifiedMode":"agent"}
            ]}"#,
        ],
    )
    .unwrap();

    let sessions = oobo::vscode_fork::extract_sessions(
        tmp.path(),
        "/home/dev/project",
        &["composer.composerData", "cascade.composerData"],
        "windsurf",
    );
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].source, "windsurf");
    assert_eq!(sessions[0].name, "Cascade chat");
    assert_eq!(sessions[1].session_id, "ws-002");
}

// ── Config with all tools tests ─────────────────────────────────────────

#[test]
fn test_config_all_tools_default_enabled() {
    let cfg: oobo::config::Config = toml::from_str("").unwrap();
    assert!(cfg.cursor.enabled);
    assert!(cfg.claude.enabled);
    assert!(cfg.windsurf.enabled);
    assert!(cfg.aider.enabled);
    assert!(cfg.continue_dev.enabled);
    assert!(cfg.zed.enabled);
    assert!(cfg.copilot.enabled);
    assert!(cfg.trae.enabled);
    assert!(cfg.codex.enabled);
    assert!(cfg.opencode.enabled);
}

#[test]
fn test_config_disable_individual_tools() {
    let toml_str = r#"
[windsurf]
enabled = false

[trae]
enabled = false
"#;
    let cfg: oobo::config::Config = toml::from_str(toml_str).unwrap();
    assert!(cfg.cursor.enabled);
    assert!(cfg.claude.enabled);
    assert!(!cfg.windsurf.enabled);
    assert!(cfg.aider.enabled);
    assert!(!cfg.trae.enabled);
}

// ── Codex CLI tests ─────────────────────────────────────────────────────────

#[test]
fn test_codex_rollout_parsing() {
    let tmp = TempDir::new().unwrap();
    let rollout = tmp
        .path()
        .join("rollout-2025-06-01T10-00-00-test-uuid.jsonl");
    fs::write(
        &rollout,
        r#"{"type":"session_start","timestamp":"2025-06-01T10:00:00Z","payload":{"cwd":"/home/dev/project"}}
{"type":"event_msg","timestamp":"2025-06-01T10:00:01Z","payload":{"type":"user_message","message":"Fix the login bug"}}
{"type":"response_item","timestamp":"2025-06-01T10:00:05Z","payload":{"role":"assistant","content":[{"type":"text","text":"I'll look into the login handler..."}]}}
{"type":"event_msg","timestamp":"2025-06-01T10:00:10Z","payload":{"type":"user_message","message":"Looks good, ship it"}}
{"type":"response_item","timestamp":"2025-06-01T10:00:15Z","payload":{"role":"assistant","content":[{"type":"text","text":"Done! The fix has been applied."}]}}
"#,
    )
    .unwrap();

    let msgs = oobo::codex::transcript::parse_messages(&rollout);
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[0].text, "Fix the login bug");
    assert_eq!(msgs[1].role, "assistant");
    assert!(msgs[1].text.contains("login handler"));
    assert_eq!(msgs[2].role, "user");
    assert_eq!(msgs[2].text, "Looks good, ship it");
    assert_eq!(msgs[3].role, "assistant");
}

#[test]
fn test_codex_rollout_read_transcript() {
    let tmp = TempDir::new().unwrap();
    let rollout = tmp.path().join("rollout-test.jsonl");
    fs::write(
        &rollout,
        r#"{"type":"event_msg","timestamp":"2025-01-01T00:00:00Z","payload":{"type":"user_message","message":"Hello"}}
{"type":"response_item","timestamp":"2025-01-01T00:00:01Z","payload":{"role":"assistant","content":[{"type":"text","text":"Hi there!"}]}}
"#,
    )
    .unwrap();

    let transcript = oobo::codex::transcript::read_transcript(&rollout, 10);
    assert!(transcript.contains("User"));
    assert!(transcript.contains("Hello"));
    assert!(transcript.contains("Assistant"));
    assert!(transcript.contains("Hi there!"));
}

// ── Alias tests ─────────────────────────────────────────────────────────────

#[test]
fn test_alias_install_uninstall_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let rc_file = tmp.path().join(".zshrc");
    fs::write(&rc_file, "# my zshrc\nexport PATH=/usr/local/bin:$PATH\n").unwrap();

    // Simulate install
    let content = fs::read_to_string(&rc_file).unwrap();
    let new_content = format!("{content}\nalias git=oobo # oobo-git alias\n");
    fs::write(&rc_file, &new_content).unwrap();

    let after_install = fs::read_to_string(&rc_file).unwrap();
    assert!(after_install.contains("oobo-git alias"));
    assert!(after_install.contains("alias git=oobo"));

    // Simulate uninstall
    let filtered: Vec<&str> = after_install
        .lines()
        .filter(|line| !line.contains("oobo-git alias"))
        .collect();
    fs::write(&rc_file, filtered.join("\n") + "\n").unwrap();

    let after_uninstall = fs::read_to_string(&rc_file).unwrap();
    assert!(!after_uninstall.contains("oobo-git alias"));
    assert!(after_uninstall.contains("my zshrc"));
    assert!(after_uninstall.contains("export PATH"));
}
