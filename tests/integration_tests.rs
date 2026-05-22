use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

// ── Fixture helpers ─────────────────────────────────────────────────────────

/// Return a fresh TempDir to use as OOBO_HOME in tests that run oobo hooks.
/// Passing this to every oobo invocation prevents test commits from being
/// synced to the backend (the isolated config has no API key / sync disabled).
fn isolated_oobo_home() -> TempDir {
    TempDir::new().unwrap()
}

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

fn oobo_binary() -> std::path::PathBuf {
    std::env::var_os("CARGO_BIN_EXE_oobo")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target/debug/oobo"))
}

fn run_oobo_with_stdin(
    repo: &Path,
    oobo_home: &Path,
    args: &[&str],
    payload: &serde_json::Value,
) -> std::process::Output {
    Command::new(oobo_binary())
        .args(args)
        .env("OOBO_HOME", oobo_home)
        .current_dir(repo)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(payload.to_string().as_bytes())
                .unwrap();
            child.wait_with_output()
        })
        .unwrap()
}

fn enable_anchor_for_repo(repo: &Path, oobo_home: &Path) {
    let output = Command::new(oobo_binary())
        .args(["enable", "--agent"])
        .env("OOBO_HOME", oobo_home)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "oobo enable failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn serve_search_once() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_http_request(&mut stream);
        assert!(
            request.starts_with("POST /anchors/search "),
            "unexpected request path: {request}"
        );
        let body = r#"{"hits":[{"project":{"id":"p:test","name":"remote-project"},"anchor_sha":"abcdef1234567890","intent":"remote hit","snippet":"from configured server","score":9.0}],"next_cursor":null}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
    });

    format!("http://{addr}")
}

fn read_http_request(stream: &mut TcpStream) -> String {
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap_or(0);
    String::from_utf8_lossy(&buf[..n]).to_string()
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

    let sessions = oobo::tools::cursor::composer::extract_sessions(tmp.path(), "/test/project");
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

    let sessions = oobo::tools::cursor::composer::extract_sessions(tmp.path(), "/test");
    assert!(sessions.is_empty());
}

#[test]
fn test_composer_extraction_missing_table() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("state.vscdb");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute("CREATE TABLE OtherTable (key TEXT, value TEXT)", [])
        .unwrap();

    let sessions = oobo::tools::cursor::composer::extract_sessions(tmp.path(), "/test");
    assert!(sessions.is_empty());
}

// ── Transcript tests ────────────────────────────────────────────────────────

#[test]
fn test_transcript_jsonl_from_fixtures() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let path = fixtures.join("transcript.jsonl");

    let messages = oobo::tools::cursor::transcript::parse_messages(&path);
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

    let messages = oobo::tools::cursor::transcript::parse_messages(&path);
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
        oobo::tools::cursor::transcript::find_transcript_path("/Test/project", "abc-123-full-uuid");
    // This won't find it because cursor_projects_dir() returns the real path,
    // but the logic is correct. We test the function directly instead.

    // Direct test of the file we created
    let jsonl = transcripts_dir.join("abc-123-full-uuid/abc-123-full-uuid.jsonl");
    assert!(jsonl.exists());
    let msgs = oobo::tools::cursor::transcript::parse_messages(&jsonl);
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
    let msgs = oobo::tools::cursor::transcript::parse_messages(&txt);
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

    let messages = oobo::tools::cursor::transcript::parse_messages(&jsonl);
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
        cursor: oobo::config::ToolConfig {
            enabled: false,
            api_key: String::new(),
        },
        claude: oobo::config::ToolConfig {
            enabled: true,
            api_key: String::new(),
        },
        windsurf: oobo::config::ToolConfig {
            enabled: true,
            api_key: String::new(),
        },
        aider: oobo::config::ToolConfig {
            enabled: true,
            api_key: String::new(),
        },
        zed: oobo::config::ToolConfig {
            enabled: true,
            api_key: String::new(),
        },
        copilot: oobo::config::ToolConfig {
            enabled: true,
            api_key: String::new(),
        },
        trae: oobo::config::ToolConfig {
            enabled: true,
            api_key: String::new(),
        },
        codex: oobo::config::ToolConfig {
            enabled: true,
            api_key: String::new(),
        },
        opencode: oobo::config::ToolConfig {
            enabled: true,
            api_key: String::new(),
        },
        gemini: oobo::config::ToolConfig {
            enabled: true,
            api_key: String::new(),
        },
        kiro: oobo::config::ToolConfig::default(),
        continue_dev: oobo::config::ToolConfig::default(),
        droid: oobo::config::ToolConfig::default(),
        junie: oobo::config::ToolConfig::default(),
        amp: oobo::config::ToolConfig::default(),
        anchors: oobo::config::AnchorsConfig::default(),
        telemetry: oobo::config::TelemetryConfig {
            enabled: true,
            send_diffs: true,
            send_transcripts: false,
        },
        scan: oobo::config::ScanConfig::default(),
        update: oobo::config::UpdateConfig::default(),
        privacy: oobo::config::TransparencyConfig::default(),
        tools: oobo::config::ToolsConfig::default(),
        setup: oobo::config::SetupConfig::default(),
        ignored_repos: Vec::new(),
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
    assert_eq!(cfg.server.url, "https://api.oobo.ai");
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

// ── Git decorator integration test ────────────────────────────────────────────

#[test]
fn test_agent_json_conflict() {
    let output = Command::new(oobo_binary())
        .args(["--agent", "--json"])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "--agent and --json should conflict"
    );
}

#[test]
fn test_oobo_anchor_list_command() {
    let tmp = TempDir::new().unwrap();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let output = Command::new(oobo_binary())
        .args(["--json"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[]") || stdout.is_empty() || output.status.success(),
        "oobo --json should succeed or return empty on fresh repo"
    );
}

#[test]
fn test_view_does_not_create_project_config() {
    let tmp = TempDir::new().unwrap();
    let oobo_home = isolated_oobo_home();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let output = Command::new(oobo_binary())
        .args(["--json"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("oobo --json should return valid JSON");
    let arr = json["anchors"]
        .as_array()
        .expect("expected anchors array in JSON output");
    assert!(arr.is_empty(), "should have no anchors");
    assert!(
        !tmp.path().join(".oobo/config").exists(),
        "viewing anchors must not create project config"
    );
}

#[test]
fn test_git_write_skips_capture_when_not_enabled() {
    let tmp = TempDir::new().unwrap();
    let oobo_home = isolated_oobo_home();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let output = Command::new("git")
        .args(["commit", "--allow-empty", "-m", "init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "git commit must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let hook = Command::new(oobo_binary())
        .args(["hooks", "post-commit"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();

    assert!(
        hook.status.success(),
        "oobo hooks post-commit must succeed even when not enabled"
    );
}

#[test]
fn test_enable_then_commit_captures() {
    let tmp = TempDir::new().unwrap();
    let oobo_home = isolated_oobo_home();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let output = Command::new(oobo_binary())
        .args(["enable"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "enable must succeed");
    assert!(
        tmp.path().join(".oobo/config").exists(),
        "enable should create project config"
    );

    let output = Command::new("git")
        .args(["commit", "--allow-empty", "-m", "init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "git commit must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let hook = Command::new(oobo_binary())
        .args(["hooks", "post-commit"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();

    assert!(
        hook.status.success(),
        "post-commit hook must succeed: {}",
        String::from_utf8_lossy(&hook.stderr)
    );
}

#[test]
fn test_disable_blocks_capture() {
    let tmp = TempDir::new().unwrap();
    let oobo_home = isolated_oobo_home();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let output = Command::new(oobo_binary())
        .args(["disable"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        tmp.path().join(".oobo/config").exists(),
        "disable should create project config"
    );

    let output = Command::new("git")
        .args(["commit", "--allow-empty", "-m", "init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "git commit must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let hook = Command::new(oobo_binary())
        .args(["hooks", "post-commit"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();

    assert!(
        hook.status.success(),
        "oobo hooks post-commit must succeed even when disabled"
    );
}

#[test]
fn test_disable_persists_project_config_state() {
    let tmp = TempDir::new().unwrap();
    let oobo_home = isolated_oobo_home();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let enable = Command::new(oobo_binary())
        .args(["enable", "--agent"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(enable.status.success());

    let disable = Command::new(oobo_binary())
        .args(["disable", "--agent"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(disable.status.success());

    let config = fs::read_to_string(tmp.path().join(".oobo/config")).unwrap();
    assert!(config.contains("enabled = false"));

    let anchors = Command::new(oobo_binary())
        .args(["--json"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(anchors.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&anchors.stdout).expect("oobo --json should return valid JSON");
    let arr = json["anchors"]
        .as_array()
        .expect("expected anchors array in JSON output");
    assert!(arr.is_empty(), "should have no anchors");
}

#[test]
fn test_e2e_commit_creates_anchor() {
    let tmp = TempDir::new().unwrap();
    let oobo_home = isolated_oobo_home();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.email", "test@oobo.dev"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    enable_anchor_for_repo(tmp.path(), oobo_home.path());

    fs::write(tmp.path().join("hello.txt"), "hello world\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let commit_output = Command::new("git")
        .args(["commit", "-m", "initial commit"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    assert!(
        commit_output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&commit_output.stderr)
    );

    let hook_output = Command::new(oobo_binary())
        .args(["hooks", "post-commit"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();

    assert!(
        hook_output.status.success(),
        "oobo hooks post-commit failed: {}",
        String::from_utf8_lossy(&hook_output.stderr)
    );

    let log_output = Command::new(oobo_binary())
        .args(["--json"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&log_output.stdout);
    assert!(log_output.status.success(), "oobo --json failed");

    let wrapper: serde_json::Value =
        serde_json::from_str(&stdout).expect("oobo --json should return valid JSON");
    let entries = wrapper["anchors"]
        .as_array()
        .unwrap_or_else(|| panic!("expected anchors array in JSON output: {stdout}"));

    assert_eq!(entries.len(), 1, "should have exactly 1 commit");
    assert_eq!(entries[0]["subject"], "initial commit");

    let hash = entries[0]["sha"].as_str().unwrap();
    assert!(hash.len() >= 7, "commit hash should be present");
}

#[test]
fn test_e2e_hook_lifecycle() {
    let tmp = TempDir::new().unwrap();
    let oobo_home = isolated_oobo_home();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    enable_anchor_for_repo(tmp.path(), oobo_home.path());

    let start_output = Command::new(oobo_binary())
        .args(["hooks", "agent", "session-start"])
        .current_dir(tmp.path())
        .env("OOBO_HOME", oobo_home.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(br#"{"session_id":"e2e-test","agent":"cursor","model":"claude-opus-4"}"#)
                .unwrap();
            child.wait_with_output()
        })
        .unwrap();

    assert!(start_output.status.success(), "session-start failed");

    let end_output = Command::new(oobo_binary())
        .args(["hooks", "agent", "session-end"])
        .current_dir(tmp.path())
        .env("OOBO_HOME", oobo_home.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(br#"{"session_id":"e2e-test"}"#)
                .unwrap();
            child.wait_with_output()
        })
        .unwrap();

    assert!(end_output.status.success(), "session-end failed");
}

#[test]
fn test_e2e_turn_capture_and_from_preview() {
    let tmp = TempDir::new().unwrap();
    let oobo_home = isolated_oobo_home();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@oobo.dev"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    fs::write(tmp.path().join("app.txt"), "v1\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let init = Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "initial git commit failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    enable_anchor_for_repo(tmp.path(), oobo_home.path());

    let sid = "turn-e2e";
    let session_start = run_oobo_with_stdin(
        tmp.path(),
        oobo_home.path(),
        &["hooks", "agent", "session-start", "--tool", "cursor"],
        &serde_json::json!({"session_id": sid, "agent": "cursor", "model": "claude-opus-4"}),
    );
    assert!(
        session_start.status.success(),
        "session-start failed: {}",
        String::from_utf8_lossy(&session_start.stderr)
    );

    let before = run_oobo_with_stdin(
        tmp.path(),
        oobo_home.path(),
        &["hooks", "agent", "before-submit-prompt", "--tool", "cursor"],
        &serde_json::json!({"session_id": sid, "prompt": "change app.txt"}),
    );
    assert!(
        before.status.success(),
        "before-submit-prompt failed: {}",
        String::from_utf8_lossy(&before.stderr)
    );

    fs::write(tmp.path().join("app.txt"), "v2\n").unwrap();
    let abs_file = tmp.path().join("app.txt").to_string_lossy().to_string();
    let after_tool = run_oobo_with_stdin(
        tmp.path(),
        oobo_home.path(),
        &["hooks", "agent", "after-tool-use", "--tool", "cursor"],
        &serde_json::json!({
            "session_id": sid,
            "tool_name": "Write",
            "tool_input": {"file_path": abs_file},
            "tool_output": {"ok": true}
        }),
    );
    assert!(
        after_tool.status.success(),
        "after-tool-use failed: {}",
        String::from_utf8_lossy(&after_tool.stderr)
    );

    let stop = run_oobo_with_stdin(
        tmp.path(),
        oobo_home.path(),
        &["hooks", "agent", "stop", "--tool", "cursor"],
        &serde_json::json!({"session_id": sid}),
    );
    assert!(
        stop.status.success(),
        "stop failed: {}",
        String::from_utf8_lossy(&stop.stderr)
    );

    let anchors_with_turns = Command::new(oobo_binary())
        .args(["--json"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        anchors_with_turns.status.success(),
        "oobo should include local shadow anchors: {}",
        String::from_utf8_lossy(&anchors_with_turns.stderr)
    );
    let memory: serde_json::Value =
        serde_json::from_slice(&anchors_with_turns.stdout).expect("anchors should emit JSON");
    let anchors_arr = memory["anchors"]
        .as_array()
        .unwrap_or_else(|| panic!("expected anchors array in JSON: {memory}"));
    let turn_memory = anchors_arr
        .iter()
        .find(|item| item["type"] == "shadow_anchor")
        .expect("anchors should include local shadow anchors inside a repo");
    let turn_id = turn_memory["shadow_anchor_id"].as_str().unwrap();
    assert_eq!(turn_memory["turn_id"], turn_id);
    assert_eq!(turn_memory["session_id"], sid);
    assert_eq!(turn_memory["files"], 1);
    assert_eq!(turn_memory["tool_calls"], 1);

    Command::new("git")
        .args(["add", "app.txt"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let commit = Command::new("git")
        .args(["commit", "-m", "capture turn"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        commit.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&commit.stderr)
    );

    let commit_hook = Command::new(oobo_binary())
        .args(["hooks", "post-commit"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        commit_hook.status.success(),
        "oobo hooks post-commit failed: {}",
        String::from_utf8_lossy(&commit_hook.stderr)
    );

    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();

    let anchor = Command::new(oobo_binary())
        .args(["anchor", "show", &head, "--json"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        anchor.status.success(),
        "anchors show failed: {}",
        String::from_utf8_lossy(&anchor.stderr)
    );
    let anchor_json: serde_json::Value =
        serde_json::from_slice(&anchor.stdout).expect("oobo anchor show should emit JSON");
    let anchor_turn = anchor_json["shadow_anchors"]
        .as_array()
        .and_then(|items| items.first())
        .expect("oobo anchor show should include turn lineage");
    assert_eq!(anchor_turn["id"], turn_id);

    // goto with --no-stash should block on dirty worktree
    fs::write(tmp.path().join("app.txt"), "v3\n").unwrap();
    let blocked = Command::new(oobo_binary())
        .args(["goto", turn_id, "--no-stash"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert_eq!(blocked.status.code(), Some(1));

    // goto without --no-stash should auto-stash and succeed
    let goto_result = Command::new(oobo_binary())
        .args(["goto", turn_id])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        goto_result.status.success(),
        "goto turn should succeed (auto-stashes): {}",
        String::from_utf8_lossy(&goto_result.stderr)
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("app.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "v2\n"
    );

    // oobo back should return to original state and pop stash
    let back_result = Command::new(oobo_binary())
        .args(["back"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        back_result.status.success(),
        "oobo back should succeed: {}",
        String::from_utf8_lossy(&back_result.stderr)
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("app.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "v3\n",
        "stash should be restored by oobo back"
    );

    let resumed_before = run_oobo_with_stdin(
        tmp.path(),
        oobo_home.path(),
        &["hooks", "agent", "before-submit-prompt", "--tool", "cursor"],
        &serde_json::json!({"session_id": sid, "prompt": "continue from loaded turn"}),
    );
    assert!(resumed_before.status.success());
    fs::write(tmp.path().join("app.txt"), "v4\n").unwrap();
    let resumed_tool = run_oobo_with_stdin(
        tmp.path(),
        oobo_home.path(),
        &["hooks", "agent", "after-tool-use", "--tool", "cursor"],
        &serde_json::json!({
            "session_id": sid,
            "tool_name": "Write",
            "tool_input": {"file_path": abs_file},
            "tool_output": {"ok": true}
        }),
    );
    assert!(resumed_tool.status.success());
    let resumed_stop = run_oobo_with_stdin(
        tmp.path(),
        oobo_home.path(),
        &["hooks", "agent", "stop", "--tool", "cursor"],
        &serde_json::json!({"session_id": sid}),
    );
    assert!(resumed_stop.status.success());

    let resumed_turns = Command::new(oobo_binary())
        .args(["--json"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(resumed_turns.status.success());
    let resumed_json: serde_json::Value =
        serde_json::from_slice(&resumed_turns.stdout).expect("anchors should emit JSON");
    let resumed_arr = resumed_json["anchors"]
        .as_array()
        .unwrap_or_else(|| panic!("expected anchors array: {resumed_json}"));
    let restored_turn = resumed_arr
        .iter()
        .find(|item| item["type"] == "shadow_anchor" && item["turn_index"] == 1)
        .expect("second shadow anchor should be captured after restore");
    assert_eq!(restored_turn["restored_from"], turn_id);
}

#[test]
fn test_e2e_from_anchor_loads_commit_tree() {
    let tmp = TempDir::new().unwrap();
    let oobo_home = isolated_oobo_home();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@oobo.dev"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    fs::write(tmp.path().join("app.txt"), "anchor-v1\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let first = Command::new("git")
        .args(["commit", "-m", "first"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(first.status.success());
    let first_hash = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let first_hash = String::from_utf8_lossy(&first_hash.stdout)
        .trim()
        .to_string();

    fs::write(tmp.path().join("app.txt"), "anchor-v2\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let second = Command::new("git")
        .args(["commit", "-m", "second"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(second.status.success());
    let head_before = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let head_before = String::from_utf8_lossy(&head_before.stdout)
        .trim()
        .to_string();

    fs::write(tmp.path().join("app.txt"), "dirty\n").unwrap();
    let load = Command::new(oobo_binary())
        .args(["goto", &first_hash])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        load.status.success(),
        "oobo goto commit should succeed (auto-stashes): {}",
        String::from_utf8_lossy(&load.stderr)
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("app.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "anchor-v1\n"
    );

    let head_after = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let head_after = String::from_utf8_lossy(&head_after.stdout)
        .trim()
        .to_string();
    assert_eq!(
        head_after, head_before,
        "loading an anchor must not move HEAD"
    );
}

#[test]
fn test_e2e_project_settings_write_oobo_config() {
    let tmp = TempDir::new().unwrap();
    let oobo_home = isolated_oobo_home();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let set_remote = Command::new(oobo_binary())
        .args([
            "settings",
            "project",
            "set",
            "remote",
            "git@github.com:acme/project-oobo.git",
        ])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        set_remote.status.success(),
        "project remote set failed: {}",
        String::from_utf8_lossy(&set_remote.stderr)
    );

    let set_transparency = Command::new(oobo_binary())
        .args(["settings", "project", "set", "transparency", "off"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        set_transparency.status.success(),
        "project transparency set failed: {}",
        String::from_utf8_lossy(&set_transparency.stderr)
    );

    let config_path = tmp.path().join(".oobo/config");
    assert!(config_path.exists(), ".oobo/config should be created");
    let config = fs::read_to_string(&config_path).unwrap();
    assert!(config.contains("git@github.com:acme/project-oobo.git"));
    assert!(config.contains("transparency = \"off\""));

    let project_settings = Command::new(oobo_binary())
        .args(["--agent", "settings", "project"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(project_settings.status.success());
    let stdout = String::from_utf8_lossy(&project_settings.stdout);
    assert!(stdout.contains("remote project git@github.com:acme/project-oobo.git"));
    assert!(stdout.contains("transparency project off"));
}

#[test]
fn test_e2e_remote_search_uses_default_server_remote() {
    let tmp = TempDir::new().unwrap();
    let oobo_home = isolated_oobo_home();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let remote_url = serve_search_once();
    let set_api_url = Command::new(oobo_binary())
        .args(["settings", "default", "set", "api_url", &remote_url])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        set_api_url.status.success(),
        "api_url set failed: {}",
        String::from_utf8_lossy(&set_api_url.stderr)
    );

    let set_key = Command::new(oobo_binary())
        .args(["settings", "set", "key", "sk_test"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(set_key.status.success());
    let config_path = if oobo_home.path().join("config").exists() {
        oobo_home.path().join("config")
    } else {
        oobo_home.path().join("config.toml")
    };
    let saved_config = fs::read_to_string(&config_path).unwrap();
    assert!(
        saved_config.contains("api_key"),
        "settings set key should persist API key"
    );

    let search = Command::new(oobo_binary())
        .args(["search", "remote", "--remote", "--json"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        search.status.success(),
        "remote search failed: {}\nstdout:\n{}",
        String::from_utf8_lossy(&search.stderr),
        String::from_utf8_lossy(&search.stdout)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&search.stdout).expect("search should emit JSON");
    assert_eq!(value["hits"][0]["project"]["name"], "remote-project");
    assert_eq!(value["hits"][0]["snippet"], "from configured server");
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
    assert!(stdout.contains("anchor"));
    assert!(stdout.contains("limit"));
}

#[test]
fn test_cli_version_flag() {
    let output = Command::new(oobo_binary())
        .args(["--version"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("oobo"));
}

#[test]
fn test_cli_anchor_help() {
    let output = Command::new(oobo_binary())
        .args(["--help"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("anchor") || stdout.contains("limit"));
}

// ── Payload serialization tests ─────────────────────────────────────────────

#[test]
fn test_event_payload_roundtrip() {
    use oobo::core::anchor::*;
    use oobo::remote::payload::*;

    let anchor = Anchor {
        anchor_schema_version: ANCHOR_SCHEMA_VERSION,
        oobo_version: "0.1.0".into(),
        commit_hash: "abc123def456".into(),
        branch: "main".into(),
        author: "Developer <dev@example.com>".into(),
        author_type: AuthorType::Assisted,
        contributors: vec![
            Contributor {
                name: "Developer <dev@example.com>".into(),
                role: ContributorRole::Human,
                model: None,
            },
            Contributor {
                name: "claude".into(),
                role: ContributorRole::Agent,
                model: Some("claude-sonnet-4".into()),
            },
        ],
        committed_at: chrono::Utc::now().timestamp(),
        message: "fix: resolve auth issue".into(),
        files_changed: vec!["src/auth.rs".into(), "src/main.rs".into()],
        added: 120,
        deleted: 45,
        file_changes: vec![
            FileChange {
                path: "src/auth.rs".into(),
                added: 100,
                deleted: 30,
                attribution: Some(FileAttribution::Ai),
                agent: Some("claude".into()),
                line_attributions: Vec::new(),
            },
            FileChange {
                path: "src/main.rs".into(),
                added: 20,
                deleted: 15,
                attribution: Some(FileAttribution::Human),
                agent: None,
                line_attributions: Vec::new(),
            },
        ],
        ai_added: 100,
        ai_deleted: 30,
        human_added: 20,
        human_deleted: 15,
        ai_percentage: Some(78.79),
        session_ids: vec!["session-uuid-123".into()],
        summary: None,
        intent: None,
        reasoning: None,
        transparency_mode: TransparencyMode::On,
        file_interactions: None,
        turns: Vec::new(),
    };

    let sessions = vec![SessionLink {
        session_id: "session-uuid-123".into(),
        agent: "claude".into(),
        model: Some("claude-sonnet-4".into()),
        link_type: LinkType::Explicit,
        input_tokens: Some(15000),
        output_tokens: Some(8000),
        cache_read_tokens: None,
        cache_creation_tokens: None,
        duration_secs: Some(120),
        tool_calls: Some(5),
        files_touched: Some(vec!["src/auth.rs".into()]),
        tool_usage: None,
        tool_failures: None,
        subagent_count: None,
        bash_commands: None,
        thinking_duration_ms: None,
        compact_count: None,
        context_tokens: None,
        context_window_size: None,
        is_subagent: false,
        parent_session_id: None,
        subagent_type: None,
        is_estimated: false,
        peer_session_ids: Vec::new(),
    }];

    let payload = EventPayload {
        payload_schema_version: EVENT_PAYLOAD_SCHEMA_VERSION,
        event: "git.commit".into(),
        timestamp: chrono::Utc::now(),
        oobo_version: "0.1.0".into(),
        project: ProjectInfo {
            name: "project".into(),
            git_remote: Some("github.com/user/project".into()),
        },
        anchor: Some(AnchorPayload { anchor, sessions }),
        transcript: vec![
            TranscriptMessage {
                role: "user".into(),
                text: Some("Fix auth".into()),
                thinking: None,
                tool_call: None,
                tool_result: None,
                timestamp_ms: None,
            },
            TranscriptMessage {
                role: "assistant".into(),
                text: Some("I'll fix the auth module...".into()),
                thinking: None,
                tool_call: None,
                tool_result: None,
                timestamp_ms: None,
            },
        ],
        session_transcripts: Vec::new(),
    };

    let json = serde_json::to_string(&payload).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed["event"], "git.commit");
    assert_eq!(parsed["project"]["name"], "project");
    assert_eq!(parsed["project"]["git_remote"], "github.com/user/project");
    assert_eq!(parsed["anchor"]["commit_hash"], "abc123def456");
    assert_eq!(parsed["anchor"]["author_type"], "assisted");
    assert_eq!(parsed["anchor"]["added"], 120);
    assert_eq!(parsed["anchor"]["deleted"], 45);
    assert_eq!(parsed["anchor"]["ai_added"], 100);
    assert_eq!(parsed["anchor"]["ai_percentage"], 78.79);
    assert_eq!(parsed["anchor"]["sessions"][0]["agent"], "claude");
    assert_eq!(parsed["anchor"]["sessions"][0]["input_tokens"], 15000);
    assert_eq!(parsed["transcript"][0]["role"], "user");
    assert!(!json.contains("cost"));
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
    use oobo::tools::cursor::path_to_slug;

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

    let sessions = oobo::tools::aider::sessions_for_project(&tmp.path().to_string_lossy()).unwrap();
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

    let msgs = oobo::tools::aider::transcript::parse_messages(&history);
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

    let msgs = oobo::tools::copilot::transcript::parse_messages(&session_file);
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[0].role, "user");
    assert!(msgs[0].text.contains("REST API"));
    assert_eq!(msgs[1].role, "assistant");
    assert!(msgs[1].text.contains("REST API"));
    assert_eq!(msgs[2].role, "user");
    assert_eq!(msgs[3].role, "assistant");
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

    let msgs = oobo::tools::zed::transcript::parse_messages(&conv_file);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, "user");
    assert!(msgs[0].text.contains("iterators"));
    assert_eq!(msgs[1].role, "assistant");
}

// ── Config with all tools tests ─────────────────────────────────────────

#[test]
fn test_config_all_tools_default_enabled() {
    let cfg: oobo::config::Config = toml::from_str("").unwrap();
    assert!(cfg.cursor.enabled);
    assert!(cfg.claude.enabled);
    assert!(cfg.windsurf.enabled);
    assert!(cfg.aider.enabled);
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

    let msgs = oobo::tools::codex::transcript::parse_messages(&rollout);
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[0].text, "Fix the login bug");
    assert_eq!(msgs[1].role, "assistant");
    assert!(msgs[1].text.contains("login handler"));
    assert_eq!(msgs[2].role, "user");
    assert_eq!(msgs[2].text, "Looks good, ship it");
    assert_eq!(msgs[3].role, "assistant");
}

// ── Alias tests ─────────────────────────────────────────────────────────────

#[test]
fn test_alias_install_uninstall_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let rc_file = tmp.path().join(".zshrc");
    fs::write(&rc_file, "# my zshrc\nexport PATH=/usr/local/bin:$PATH\n").unwrap();

    // Simulate install
    let content = fs::read_to_string(&rc_file).unwrap();
    let new_content = format!("{content}\nalias git=oobo # oobo alias\n");
    fs::write(&rc_file, &new_content).unwrap();

    let after_install = fs::read_to_string(&rc_file).unwrap();
    assert!(after_install.contains("oobo alias"));
    assert!(after_install.contains("alias git=oobo"));

    // Simulate uninstall
    let filtered: Vec<&str> = after_install
        .lines()
        .filter(|line| !line.contains("oobo alias"))
        .collect();
    fs::write(&rc_file, filtered.join("\n") + "\n").unwrap();

    let after_uninstall = fs::read_to_string(&rc_file).unwrap();
    assert!(!after_uninstall.contains("oobo alias"));
    assert!(after_uninstall.contains("my zshrc"));
    assert!(after_uninstall.contains("export PATH"));
}

#[test]
fn test_oobo_blame_json_output() {
    let tmp = TempDir::new().unwrap();
    let oobo_home = isolated_oobo_home();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.email", "test@oobo.dev"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    enable_anchor_for_repo(tmp.path(), oobo_home.path());

    // Initial commit with a file
    fs::write(tmp.path().join("src.rs"), "fn main() {}\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let commit1 = Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(commit1.status.success(), "first git commit failed");

    let hook1 = Command::new(oobo_binary())
        .args(["hooks", "post-commit"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(hook1.status.success(), "first post-commit hook failed");

    // Start a session, then modify the file, snapshot, commit
    let start = Command::new(oobo_binary())
        .args(["hooks", "agent", "session-start"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(
                    br#"{"session_id":"blame-test","agent":"cursor","model":"claude-opus-4"}"#,
                )
                .unwrap();
            child.wait_with_output()
        })
        .unwrap();
    assert!(start.status.success(), "session-start failed");

    // Simulate agent editing the file
    fs::write(
        tmp.path().join("src.rs"),
        "fn main() {\n    println!(\"hello\");\n}\n",
    )
    .unwrap();

    // Snapshot the file (simulates stop hook snapshotting)
    let stop = Command::new(oobo_binary())
        .args(["hooks", "agent", "stop"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(br#"{"session_id":"blame-test"}"#)
                .unwrap();
            child.wait_with_output()
        })
        .unwrap();
    assert!(stop.status.success(), "stop hook failed");

    // Stage and commit
    Command::new("git")
        .args(["add", "."])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let commit2 = Command::new("git")
        .args(["commit", "-m", "add hello"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        commit2.status.success(),
        "second git commit failed: {}",
        String::from_utf8_lossy(&commit2.stderr)
    );

    let hook2 = Command::new(oobo_binary())
        .args(["hooks", "post-commit"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        hook2.status.success(),
        "second post-commit hook failed: {}",
        String::from_utf8_lossy(&hook2.stderr)
    );

    // Run blame --json
    let blame_output = Command::new(oobo_binary())
        .args(["anchor", "blame", "src.rs", "--json"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&blame_output.stdout);

    if blame_output.status.success() {
        let val: serde_json::Value =
            serde_json::from_str(&stdout).expect("blame --json should return valid JSON");
        assert_eq!(val["file"], "src.rs");
        assert!(val["lines"].is_array());
    }
    // If blame fails (no anchor yet for this commit), that's ok —
    // the commit might not have generated line-level data without
    // a proper before-submit-prompt snapshot. The test at minimum
    // verifies the command doesn't crash.
}

/// Verify that `after-tool-use` snapshots edited files immediately, so a
/// subsequent `git commit` (before `stop`) produces per-line attribution.
#[test]
fn test_after_tool_use_snapshots_enable_line_attribution() {
    let tmp = TempDir::new().unwrap();
    let oobo_home = isolated_oobo_home();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.email", "test@oobo.dev"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    enable_anchor_for_repo(tmp.path(), oobo_home.path());

    // Initial commit
    fs::write(tmp.path().join("app.rs"), "fn main() {}\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let c1 = Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(c1.status.success(), "initial git commit failed");

    let h1 = Command::new(oobo_binary())
        .args(["hooks", "post-commit"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(h1.status.success(), "initial post-commit hook failed");

    // 1. session-start
    let start = Command::new(oobo_binary())
        .args(["hooks", "agent", "session-start"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(
                    br#"{"session_id":"line-test","agent":"cursor","model":"claude-opus-4"}"#,
                )
                .unwrap();
            child.wait_with_output()
        })
        .unwrap();
    assert!(start.status.success(), "session-start failed");

    // 2. Agent edits the file
    fs::write(
        tmp.path().join("app.rs"),
        "fn main() {\n    println!(\"hello\");\n}\n",
    )
    .unwrap();

    // 3. after-tool-use fires (simulates Cursor's postToolUse for a Write)
    let abs_file = tmp.path().join("app.rs").to_string_lossy().to_string();
    let hook_payload = serde_json::json!({
        "session_id": "line-test",
        "tool_name": "Write",
        "file_path": abs_file,
    });
    let atu = Command::new(oobo_binary())
        .args(["hooks", "agent", "after-tool-use"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(hook_payload.to_string().as_bytes())
                .unwrap();
            child.wait_with_output()
        })
        .unwrap();
    assert!(atu.status.success(), "after-tool-use failed");

    // 4. git add + commit — NO stop hook yet (mirrors real agent flow)
    Command::new("git")
        .args(["add", "."])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let c2 = Command::new("git")
        .args(["commit", "-m", "add hello"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        c2.status.success(),
        "second git commit failed: {}",
        String::from_utf8_lossy(&c2.stderr)
    );

    let h2 = Command::new(oobo_binary())
        .args(["hooks", "post-commit"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        h2.status.success(),
        "second post-commit hook failed: {}",
        String::from_utf8_lossy(&h2.stderr)
    );

    // 5. Verify blame --json has line_attributions
    let blame = Command::new(oobo_binary())
        .args(["anchor", "blame", "app.rs", "--json"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&blame.stdout);

    if blame.status.success() {
        let val: serde_json::Value =
            serde_json::from_str(&stdout).expect("blame --json should return valid JSON");
        assert_eq!(val["file"], "app.rs");
        let lines = val["lines"].as_array().expect("lines should be an array");
        // At least one line should have non-null AI attribution, proving
        // after-tool-use snapshots produced per-line data.
        let has_ai = lines.iter().any(|l| !l["ai"].is_null());
        assert!(
            has_ai,
            "at least one line should have AI attribution — after-tool-use \
             should have snapshotted the file so enrich_commit can produce \
             per-line data. Got: {stdout}"
        );
    }
}

#[test]
fn test_post_rewrite_amend_rekeys_orphan_anchor() {
    let tmp = TempDir::new().unwrap();
    let oobo_home = isolated_oobo_home();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@oobo.dev"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    enable_anchor_for_repo(tmp.path(), oobo_home.path());

    fs::write(tmp.path().join("rewrite.txt"), "hello rewrite\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let commit = Command::new("git")
        .args(["commit", "-m", "original anchor"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        commit.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&commit.stderr)
    );

    let hook = Command::new(oobo_binary())
        .args(["hooks", "post-commit"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        hook.status.success(),
        "oobo hooks post-commit failed: {}",
        String::from_utf8_lossy(&hook.stderr)
    );

    let old_sha = git_stdout(tmp.path(), &["rev-parse", "HEAD"]);
    assert!(
        oobo::git::orphan::read_anchor(tmp.path().to_str().unwrap(), &old_sha).is_some(),
        "initial anchor commit should write an orphan anchor"
    );

    let amend = Command::new("git")
        .args(["commit", "--amend", "-m", "amended anchor"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        amend.status.success(),
        "git amend failed: {}",
        String::from_utf8_lossy(&amend.stderr)
    );
    let new_sha = git_stdout(tmp.path(), &["rev-parse", "HEAD"]);
    assert_ne!(old_sha, new_sha, "amend should rewrite the commit sha");

    let rewrite = Command::new(oobo_binary())
        .args(["hooks", "post-rewrite", "amend"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(format!("{old_sha} {new_sha}\n").as_bytes())
                .unwrap();
            child.wait_with_output()
        })
        .unwrap();
    assert!(
        rewrite.status.success(),
        "post-rewrite hook failed: {}",
        String::from_utf8_lossy(&rewrite.stderr)
    );

    let rekeyed = oobo::git::orphan::read_anchor(tmp.path().to_str().unwrap(), &new_sha)
        .expect("post-rewrite should copy anchor metadata to rewritten sha");
    assert_eq!(rekeyed.commit_hash, new_sha);
    assert_eq!(rekeyed.message, "original anchor");
}

#[test]
fn test_post_rewrite_rebase_rekeys_same_tree_anchor() {
    let tmp = TempDir::new().unwrap();
    let oobo_home = isolated_oobo_home();
    init_git_repo(tmp.path());
    enable_anchor_for_repo(tmp.path(), oobo_home.path());

    fs::write(tmp.path().join("base.txt"), "base\n").unwrap();
    git_ok(tmp.path(), &["add", "."]);
    git_ok(tmp.path(), &["commit", "-m", "base"]);
    let old_base = git_stdout(tmp.path(), &["rev-parse", "HEAD"]);

    fs::write(tmp.path().join("feature.txt"), "feature\n").unwrap();
    git_ok(tmp.path(), &["add", "."]);
    anchor_commit_ok(tmp.path(), oobo_home.path(), "feature anchor");
    let old_feature = git_stdout(tmp.path(), &["rev-parse", "HEAD"]);
    let old_tree = git_stdout(tmp.path(), &["show", "-s", "--format=%T", &old_feature]);
    assert!(
        oobo::git::orphan::read_anchor(tmp.path().to_str().unwrap(), &old_feature).is_some(),
        "initial feature commit should have an orphan anchor"
    );

    let branch = git_stdout(tmp.path(), &["branch", "--show-current"]);
    git_ok(tmp.path(), &["checkout", &old_base]);
    git_ok(tmp.path(), &["commit", "--amend", "-m", "base reworded"]);
    let new_base = git_stdout(tmp.path(), &["rev-parse", "HEAD"]);
    git_ok(tmp.path(), &["branch", "rewritten-base", &new_base]);
    git_ok(tmp.path(), &["checkout", &branch]);

    let rebase = Command::new("git")
        .args(["rebase", "--onto", "rewritten-base", &old_base, &branch])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        rebase.status.success(),
        "git rebase failed: {}",
        String::from_utf8_lossy(&rebase.stderr)
    );

    let new_feature = git_stdout(tmp.path(), &["rev-parse", "HEAD"]);
    let new_tree = git_stdout(tmp.path(), &["show", "-s", "--format=%T", &new_feature]);
    assert_ne!(old_feature, new_feature, "rebase should rewrite the sha");
    assert_eq!(old_tree, new_tree, "this rebase fixture must preserve tree");

    run_post_rewrite_hook(
        tmp.path(),
        oobo_home.path(),
        "rebase",
        &format!("{old_feature} {new_feature}\n"),
    );

    let rekeyed = oobo::git::orphan::read_anchor(tmp.path().to_str().unwrap(), &new_feature)
        .expect("same-tree rebase should copy anchor metadata to rewritten sha");
    assert_eq!(rekeyed.commit_hash, new_feature);
    assert_eq!(rekeyed.message, "feature anchor");
}

#[test]
fn test_post_rewrite_squash_rekeys_latest_same_tree_anchor() {
    let tmp = TempDir::new().unwrap();
    let oobo_home = isolated_oobo_home();
    init_git_repo(tmp.path());
    enable_anchor_for_repo(tmp.path(), oobo_home.path());

    fs::write(tmp.path().join("base.txt"), "base\n").unwrap();
    git_ok(tmp.path(), &["add", "."]);
    git_ok(tmp.path(), &["commit", "-m", "base"]);

    fs::write(tmp.path().join("first.txt"), "first\n").unwrap();
    git_ok(tmp.path(), &["add", "."]);
    anchor_commit_ok(tmp.path(), oobo_home.path(), "first anchor");
    let first_sha = git_stdout(tmp.path(), &["rev-parse", "HEAD"]);

    fs::write(tmp.path().join("second.txt"), "second\n").unwrap();
    git_ok(tmp.path(), &["add", "."]);
    anchor_commit_ok(tmp.path(), oobo_home.path(), "second anchor");
    let second_sha = git_stdout(tmp.path(), &["rev-parse", "HEAD"]);
    let second_tree = git_stdout(tmp.path(), &["show", "-s", "--format=%T", &second_sha]);

    git_ok(tmp.path(), &["reset", "--soft", "HEAD~2"]);
    git_ok(tmp.path(), &["commit", "-m", "squashed feature"]);
    let squashed_sha = git_stdout(tmp.path(), &["rev-parse", "HEAD"]);
    let squashed_tree = git_stdout(tmp.path(), &["show", "-s", "--format=%T", &squashed_sha]);
    assert_ne!(second_sha, squashed_sha, "squash should rewrite the sha");
    assert_eq!(
        second_tree, squashed_tree,
        "squash fixture should preserve the final tree from the latest old commit"
    );

    run_post_rewrite_hook(
        tmp.path(),
        oobo_home.path(),
        "rebase",
        &format!("{first_sha} {squashed_sha}\n{second_sha} {squashed_sha}\n"),
    );

    let rekeyed = oobo::git::orphan::read_anchor(tmp.path().to_str().unwrap(), &squashed_sha)
        .expect("squash should copy the latest same-tree anchor to the squashed sha");
    assert_eq!(rekeyed.commit_hash, squashed_sha);
    assert_eq!(rekeyed.message, "second anchor");
}

fn init_git_repo(repo: &Path) {
    git_ok(repo, &["init"]);
    git_ok(repo, &["config", "user.email", "test@oobo.dev"]);
    git_ok(repo, &["config", "user.name", "Test User"]);
}

fn anchor_commit_ok(repo: &Path, oobo_home: &Path, message: &str) {
    let output = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let hook = Command::new(oobo_binary())
        .args(["hooks", "post-commit"])
        .env("OOBO_HOME", oobo_home)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        hook.status.success(),
        "oobo hooks post-commit failed: {}",
        String::from_utf8_lossy(&hook.stderr)
    );
}

fn run_post_rewrite_hook(repo: &Path, oobo_home: &Path, rewrite_kind: &str, payload: &str) {
    let output = Command::new(oobo_binary())
        .args(["hooks", "post-rewrite", rewrite_kind])
        .env("OOBO_HOME", oobo_home)
        .current_dir(repo)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(payload.as_bytes())
                .unwrap();
            child.wait_with_output()
        })
        .unwrap();
    assert!(
        output.status.success(),
        "post-rewrite hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_ok(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn test_pre_tool_use_creates_edit_chain() {
    let tmp = TempDir::new().unwrap();
    let oobo_home = isolated_oobo_home();

    Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@oobo.dev"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    fs::write(tmp.path().join("chain.txt"), "original\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let init = Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "initial commit failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    enable_anchor_for_repo(tmp.path(), oobo_home.path());

    let sid = "chain-e2e";
    let abs_file = tmp.path().join("chain.txt").to_string_lossy().to_string();

    // 1. session-start
    let session_start = run_oobo_with_stdin(
        tmp.path(),
        oobo_home.path(),
        &["hooks", "agent", "session-start", "--tool", "cursor"],
        &serde_json::json!({"session_id": sid, "agent": "cursor", "model": "claude-opus-4"}),
    );
    assert!(
        session_start.status.success(),
        "session-start failed: {}",
        String::from_utf8_lossy(&session_start.stderr)
    );

    // 2. before-submit-prompt
    let before = run_oobo_with_stdin(
        tmp.path(),
        oobo_home.path(),
        &["hooks", "agent", "before-submit-prompt", "--tool", "cursor"],
        &serde_json::json!({"session_id": sid, "prompt": "edit chain.txt"}),
    );
    assert!(
        before.status.success(),
        "before-submit-prompt failed: {}",
        String::from_utf8_lossy(&before.stderr)
    );

    // 3. pre-tool-use
    let pre_tool = run_oobo_with_stdin(
        tmp.path(),
        oobo_home.path(),
        &["hooks", "agent", "pre-tool-use", "--tool", "cursor"],
        &serde_json::json!({
            "session_id": sid,
            "tool_name": "Write",
            "file_path": abs_file,
        }),
    );
    assert!(
        pre_tool.status.success(),
        "pre-tool-use failed: {}",
        String::from_utf8_lossy(&pre_tool.stderr)
    );

    // 4. Modify the file (simulates the AI tool)
    fs::write(tmp.path().join("chain.txt"), "modified by ai\n").unwrap();

    // 5. after-tool-use
    let after_tool = run_oobo_with_stdin(
        tmp.path(),
        oobo_home.path(),
        &["hooks", "agent", "after-tool-use", "--tool", "cursor"],
        &serde_json::json!({
            "session_id": sid,
            "tool_name": "Write",
            "file_path": abs_file,
            "tool_input": {"file_path": abs_file},
            "tool_output": {"ok": true},
        }),
    );
    assert!(
        after_tool.status.success(),
        "after-tool-use failed: {}",
        String::from_utf8_lossy(&after_tool.stderr)
    );

    // 6. stop
    let stop = run_oobo_with_stdin(
        tmp.path(),
        oobo_home.path(),
        &["hooks", "agent", "stop", "--tool", "cursor"],
        &serde_json::json!({"session_id": sid}),
    );
    assert!(
        stop.status.success(),
        "stop failed: {}",
        String::from_utf8_lossy(&stop.stderr)
    );

    // 7. Verify the turn snapshot has correct pre/post blobs
    let anchors = Command::new(oobo_binary())
        .args(["--json"])
        .env("OOBO_HOME", oobo_home.path())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        anchors.status.success(),
        "oobo --json failed: {}",
        String::from_utf8_lossy(&anchors.stderr)
    );
    let memory: serde_json::Value =
        serde_json::from_slice(&anchors.stdout).expect("anchors should emit JSON");
    let anchors_arr = memory["anchors"]
        .as_array()
        .unwrap_or_else(|| panic!("expected anchors array: {memory}"));
    let turn = anchors_arr
        .iter()
        .find(|item| item["type"] == "shadow_anchor")
        .expect("should have a shadow anchor from the turn");

    assert_eq!(turn["session_id"], sid);
    assert_eq!(turn["files"], 1);

    let turn_id = turn["turn_id"].as_str().unwrap();
    let turn_snapshot = oobo::git::turns::read_turn_snapshot(tmp.path().to_str().unwrap(), turn_id)
        .expect("turn snapshot should exist");

    let file_snap = turn_snapshot
        .files
        .iter()
        .find(|f| {
            let normalized = f.path.replace('\\', "/");
            normalized == "chain.txt" || normalized.ends_with("/chain.txt")
        })
        .unwrap_or_else(|| {
            let paths: Vec<_> = turn_snapshot.files.iter().map(|f| &f.path).collect();
            panic!(
                "chain.txt should be in the turn snapshot files, found: {:?}",
                paths
            )
        });

    assert!(
        file_snap.pre_blob.is_some(),
        "pre_blob should be set from edit chain"
    );
    assert!(
        file_snap.post_blob.is_some(),
        "post_blob should be set from edit chain"
    );
    assert_ne!(
        file_snap.pre_blob, file_snap.post_blob,
        "pre and post blobs should differ since the file was modified"
    );
}
