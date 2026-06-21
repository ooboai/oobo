use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

const SPECS: &[(&str, &str)] = &[
    (
        "00-global-flags.md",
        include_str!("cli-spec/00-global-flags.md"),
    ),
    ("01-bare.md", include_str!("cli-spec/01-bare.md")),
    ("02-anchors.md", include_str!("cli-spec/02-anchors.md")),
    ("03-blame.md", include_str!("cli-spec/03-blame.md")),
    ("04-recall.md", include_str!("cli-spec/04-recall.md")),
    (
        "05-enable-disable.md",
        include_str!("cli-spec/05-enable-disable.md"),
    ),
    ("07-setup.md", include_str!("cli-spec/07-setup.md")),
    ("08-settings.md", include_str!("cli-spec/08-settings.md")),
    ("09-update.md", include_str!("cli-spec/09-update.md")),
    ("12-hooks.md", include_str!("cli-spec/12-hooks.md")),
    ("13-env-vars.md", include_str!("cli-spec/13-env-vars.md")),
    (
        "14-turns-from.md",
        include_str!("cli-spec/14-turns-from.md"),
    ),
    ("15-delta.md", include_str!("cli-spec/15-delta.md")),
    ("16-help.md", include_str!("cli-spec/16-help.md")),
    (
        "17-code-search.md",
        include_str!("cli-spec/17-code-search.md"),
    ),
    ("18-mcp.md", include_str!("cli-spec/18-mcp.md")),
    ("19-sessions.md", include_str!("cli-spec/19-sessions.md")),
];

const RESERVED_COMMANDS: &[&str] = &[
    "anchor", "anchors", "search", "recall", "enable", "disable", "setup", "settings", "update",
    "hooks", "goto", "back", "mcp", "session", "sessions",
];

const PUBLIC_HELP_COMMANDS: &[&str] = &[
    "anchors", "anchor", "delta", "goto", "back", "blame", "search", "recall", "settings",
    "enable", "disable", "setup", "mcp", "help", "update", "session", "sessions",
];

#[derive(Debug, Clone)]
struct Invocation {
    file: &'static str,
    line: usize,
    command: String,
}

#[derive(Debug, Clone, Copy)]
enum JsonShape {
    Composite,
}

#[derive(Debug, Clone, Copy)]
struct CliCase {
    command: &'static str,
    expected_code: i32,
    json_shape: Option<JsonShape>,
    assert_agent_plain: bool,
    stdout_contains: &'static [&'static str],
    stderr_contains: &'static [&'static str],
    compare_to_git: Option<&'static [&'static str]>,
}

#[test]
fn cli_spec_invocation_blocks_are_parseable() {
    let invocations = parse_invocations();
    assert!(
        invocations.len() >= 40,
        "expected many cli-spec invocations, got {}",
        invocations.len()
    );

    for inv in invocations {
        let parsed = split_command(&inv.command).unwrap_or_else(|e| {
            panic!(
                "{}:{} cannot parse {:?}: {e}",
                inv.file, inv.line, inv.command
            )
        });
        assert!(
            !parsed.is_empty(),
            "{}:{} parsed to empty argv",
            inv.file,
            inv.line
        );
    }
}

#[test]
fn top_level_help_keeps_public_command_footprint_small() {
    let oobo_home = TempDir::new().unwrap();
    let output = Command::new(oobo_binary())
        .arg("--help")
        .env("OOBO_HOME", oobo_home.path())
        .output()
        .expect("run oobo --help");
    assert!(output.status.success(), "oobo --help should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let public_help = stdout.split("OPTIONS:").next().unwrap_or(stdout.as_ref());
    let mut commands = std::collections::BTreeSet::new();

    for line in public_help.lines() {
        if !line.starts_with("  ") {
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with('-') || trimmed.is_empty() {
            continue;
        }
        let command_column = trimmed.split("  ").next().unwrap_or(trimmed).trim();
        for command in command_column.split(',').map(str::trim) {
            if command.chars().all(|c| c.is_ascii_lowercase()) {
                commands.insert(command.to_string());
            }
        }
    }

    let expected = PUBLIC_HELP_COMMANDS
        .iter()
        .map(|command| command.to_string())
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(
        commands, expected,
        "top-level help exposed an unexpected command footprint"
    );
}

#[test]
fn recall_help_exposes_real_remote_flags() {
    let oobo_home = TempDir::new().unwrap();
    let output = Command::new(oobo_binary())
        .args(["recall", "--help"])
        .env("OOBO_HOME", oobo_home.path())
        .output()
        .expect("run oobo recall --help");
    assert!(output.status.success(), "oobo recall --help should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--remote"),
        "recall help should expose remote search"
    );
    assert!(
        stdout.contains("--both"),
        "recall help should expose local+remote search"
    );
}

#[test]
fn reserved_command_footprint_matches_spec() {
    let mut documented = std::collections::BTreeSet::new();

    for inv in parse_invocations() {
        let Ok(tokens) = split_command(&inv.command) else {
            continue;
        };
        let Some(oobo_idx) = tokens.iter().position(|t| t == "oobo") else {
            continue;
        };
        if let Some(verb) = first_non_flag_after_oobo(&tokens[oobo_idx + 1..]) {
            if RESERVED_COMMANDS.contains(&verb.as_str()) {
                documented.insert(verb);
            }
        }
    }

    for expected in RESERVED_COMMANDS {
        assert!(
            documented.contains(*expected),
            "reserved command {expected:?} is not represented in cli-spec invocations"
        );
    }
}

#[test]
fn safe_cli_spec_invocations_smoke_run() {
    let cases = safe_cli_spec_cases();
    assert!(!cases.is_empty(), "no safe CLI spec cases selected");

    for case in cases {
        let command = case.command;
        let mut tokens = split_command(command).expect("CLI spec case should parse");
        let envs = extract_leading_env_assignments(&mut tokens);
        assert_eq!(tokens.first().map(String::as_str), Some("oobo"));

        let sandbox = Sandbox::new();
        let output = run_oobo(&sandbox, &tokens[1..], &envs);
        let code = output.status.code().unwrap_or(1);
        assert_eq!(
            code,
            case.expected_code,
            "unexpected exit code for {command:?}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        for expected in case.stdout_contains {
            assert!(
                stdout.contains(expected),
                "{command:?} stdout should contain {expected:?}, got:\n{stdout}"
            );
        }
        for expected in case.stderr_contains {
            assert!(
                stderr.contains(expected),
                "{command:?} stderr should contain {expected:?}, got:\n{stderr}"
            );
        }

        if let Some(shape) = case.json_shape {
            assert_json_stdout(command, &stdout, shape);
        }

        if case.assert_agent_plain {
            assert_agent_plain_output(command, &stdout);
        }

        if let Some(git_args) = case.compare_to_git {
            let git_output = run_git_capture(&sandbox.repo, git_args);
            assert_eq!(
                output.status.code(),
                git_output.status.code(),
                "{command:?} exit code drifted from git {git_args:?}"
            );
            assert_eq!(
                output.stdout, git_output.stdout,
                "{command:?} stdout drifted from git {git_args:?}"
            );
            assert_eq!(
                output.stderr, git_output.stderr,
                "{command:?} stderr drifted from git {git_args:?}"
            );
        }
    }
}

#[test]
fn global_output_flags_are_position_independent_for_search() {
    let sandbox = Sandbox::new();

    for flag in ["--agent", "--json"] {
        let root_first = run_oobo(&sandbox, &[flag, "search", "test"], &[]);
        let command_first = run_oobo(&sandbox, &["search", "test", flag], &[]);

        assert_eq!(
            root_first.status.code(),
            command_first.status.code(),
            "{flag} exit code should be position-independent"
        );
        let root_stdout = normalize_position_independent_stdout(flag, &root_first.stdout);
        let command_stdout = normalize_position_independent_stdout(flag, &command_first.stdout);
        assert_eq!(
            root_stdout, command_stdout,
            "{flag} stdout should be position-independent"
        );
        assert_eq!(
            root_first.stderr, command_first.stderr,
            "{flag} stderr should be position-independent"
        );
    }
}

fn normalize_position_independent_stdout(flag: &str, stdout: &[u8]) -> Vec<u8> {
    if flag != "--agent" {
        return stdout.to_vec();
    }

    let text = String::from_utf8_lossy(stdout);
    regex::Regex::new(r"\b\d+(?:mo|[smhdwy])\b")
        .expect("relative-time regex should compile")
        .replace_all(&text, "<rel>")
        .as_bytes()
        .to_vec()
}

fn extract_leading_env_assignments(tokens: &mut Vec<String>) -> Vec<(String, String)> {
    let mut envs = Vec::new();
    while tokens
        .first()
        .is_some_and(|t| t.contains('=') && !t.starts_with('-'))
    {
        let assignment = tokens.remove(0);
        let (key, value) = assignment
            .split_once('=')
            .expect("checked assignment shape");
        envs.push((key.to_string(), value.to_string()));
    }
    envs
}

fn run_oobo(
    sandbox: &Sandbox,
    args: &[impl AsRef<std::ffi::OsStr> + std::fmt::Debug],
    envs: &[(String, String)],
) -> std::process::Output {
    let mut cmd = Command::new(oobo_binary());
    cmd.args(args)
        .current_dir(&sandbox.repo)
        .env("OOBO_HOME", sandbox.oobo_home.path())
        .env("OOBO_TEST", "1")
        .env("NO_COLOR", "1");
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output()
        .unwrap_or_else(|e| panic!("failed to run oobo {args:?}: {e}"))
}

fn assert_json_stdout(command: &str, stdout: &str, shape: JsonShape) {
    let value = serde_json::from_str::<serde_json::Value>(stdout.trim())
        .unwrap_or_else(|e| panic!("{command:?} should emit valid JSON, got {e}: {stdout}"));

    match shape {
        JsonShape::Composite => assert!(
            value.is_object() || value.is_array(),
            "{command:?} should emit a JSON object or array, got {value}"
        ),
    }
}

fn assert_agent_plain_output(command: &str, stdout: &str) {
    assert!(
        !stdout.contains('\u{1b}'),
        "{command:?} emitted ANSI escape codes in agent output"
    );
    assert!(
        !stdout.contains('\t'),
        "{command:?} emitted tabs in agent output"
    );
    assert!(
        stdout.is_ascii(),
        "{command:?} emitted non-ASCII in agent output: {stdout:?}"
    );
}

fn parse_invocations() -> Vec<Invocation> {
    let mut out = Vec::new();

    for (file, content) in SPECS {
        let lines: Vec<&str> = content.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if !line.trim_start().contains("### Invocation") {
                continue;
            }

            if let Some(command) = invocation_on_same_line(line).or_else(|| {
                lines
                    .iter()
                    .skip(idx + 1)
                    .take(6)
                    .find_map(|candidate| command_from_backtick_line(candidate))
            }) {
                out.push(Invocation {
                    file,
                    line: idx + 1,
                    command,
                });
            }
        }
    }

    out
}

fn invocation_on_same_line(line: &str) -> Option<String> {
    let (_, rest) = line.split_once(':')?;
    command_from_backtick_line(rest)
}

fn command_from_backtick_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with("```") {
        return None;
    }
    let start = trimmed.find('`')?;
    let end = trimmed[start + 1..].find('`')? + start + 1;
    let command = trimmed[start + 1..end].trim();
    if command.is_empty() {
        return None;
    }
    Some(command.to_string())
}

fn first_non_flag_after_oobo(tokens: &[String]) -> Option<String> {
    let mut i = 0;
    while i < tokens.len() {
        let token = &tokens[i];
        if token == "--" {
            return tokens.get(i + 1).cloned();
        }
        if token.starts_with('-') {
            i += if flag_takes_value(token) { 2 } else { 1 };
            continue;
        }
        return Some(token.clone());
    }
    None
}

fn flag_takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "--tool" | "--since" | "--limit" | "--remote" | "--key" | "-n"
    )
}

fn safe_cli_spec_cases() -> Vec<CliCase> {
    vec![
        CliCase {
            command: "oobo --help",
            expected_code: 0,
            json_shape: None,
            assert_agent_plain: false,
            stdout_contains: &["Usage:", "Commands", "Options:"],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo --version",
            expected_code: 0,
            json_shape: None,
            assert_agent_plain: true,
            stdout_contains: &["oobo "],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo --version --json",
            expected_code: 0,
            json_shape: Some(JsonShape::Composite),
            assert_agent_plain: false,
            stdout_contains: &[
                "\"name\": \"oobo\"",
                "\"version\":",
                "\"commit\":",
                "\"built_at\":",
            ],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo --agent",
            expected_code: 0,
            json_shape: None,
            assert_agent_plain: true,
            stdout_contains: &[],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo --json",
            expected_code: 0,
            json_shape: Some(JsonShape::Composite),
            assert_agent_plain: false,
            stdout_contains: &[],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo --agent --json",
            expected_code: 2,
            json_shape: None,
            assert_agent_plain: false,
            stdout_contains: &[],
            stderr_contains: &["--agent", "--json"],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo anchor show --help",
            expected_code: 0,
            json_shape: None,
            assert_agent_plain: false,
            stdout_contains: &[],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo blame --help",
            expected_code: 0,
            json_shape: None,
            assert_agent_plain: false,
            stdout_contains: &[],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo setup --help",
            expected_code: 0,
            json_shape: None,
            assert_agent_plain: false,
            stdout_contains: &[],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo update --help",
            expected_code: 0,
            json_shape: None,
            assert_agent_plain: false,
            stdout_contains: &[],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo hooks --help",
            expected_code: 0,
            json_shape: None,
            assert_agent_plain: false,
            stdout_contains: &[],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo settings --agent",
            expected_code: 0,
            json_shape: None,
            assert_agent_plain: true,
            stdout_contains: &[],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo settings --json",
            expected_code: 0,
            json_shape: Some(JsonShape::Composite),
            assert_agent_plain: false,
            stdout_contains: &[],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo recall auth --agent",
            expected_code: 0,
            json_shape: None,
            assert_agent_plain: true,
            stdout_contains: &[],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo recall auth --json",
            expected_code: 0,
            json_shape: Some(JsonShape::Composite),
            assert_agent_plain: false,
            stdout_contains: &[],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo enable --agent",
            expected_code: 0,
            json_shape: None,
            assert_agent_plain: true,
            stdout_contains: &["enabled"],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo disable --agent",
            expected_code: 0,
            json_shape: None,
            assert_agent_plain: true,
            stdout_contains: &["disabled"],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo hooks agent fart --tool cursor",
            expected_code: 0,
            json_shape: None,
            assert_agent_plain: true,
            stdout_contains: &[],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo hooks post-merge",
            expected_code: 0,
            json_shape: None,
            assert_agent_plain: true,
            stdout_contains: &[],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo hooks post-rewrite",
            expected_code: 0,
            json_shape: None,
            assert_agent_plain: true,
            stdout_contains: &[],
            stderr_contains: &[],
            compare_to_git: None,
        },
        CliCase {
            command: "oobo status",
            expected_code: 2,
            json_shape: None,
            assert_agent_plain: false,
            stdout_contains: &[],
            stderr_contains: &["unrecognized subcommand"],
            compare_to_git: None,
        },
        CliCase {
            command: "CURSOR_AGENT=1 oobo --agent",
            expected_code: 0,
            json_shape: None,
            assert_agent_plain: true,
            stdout_contains: &[],
            stderr_contains: &[],
            compare_to_git: None,
        },
    ]
}

fn split_command(command: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote: Option<char> = None;

    while let Some(ch) = chars.next() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                } else if ch == '\\' {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                } else {
                    current.push(ch);
                }
            }
            None => match ch {
                '\'' | '"' => quote = Some(ch),
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                ' ' | '\t' => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                '|' | '>' | '<' => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                    tokens.push(ch.to_string());
                    if matches!(chars.peek(), Some('>' | '<')) {
                        tokens.last_mut().unwrap().push(chars.next().unwrap());
                    }
                }
                _ => current.push(ch),
            },
        }
    }

    if let Some(q) = quote {
        return Err(format!("unclosed quote {q}"));
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Ok(tokens)
}

fn oobo_binary() -> std::path::PathBuf {
    std::env::var_os("CARGO_BIN_EXE_oobo")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target/debug/oobo"))
}

struct Sandbox {
    _tmp: TempDir,
    oobo_home: TempDir,
    repo: std::path::PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let oobo_home = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init"]);
        git(&repo, &["config", "user.email", "spec@oobo.dev"]);
        git(&repo, &["config", "user.name", "Spec Test"]);
        std::fs::write(repo.join("README.md"), "spec\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);

        let sandbox = Self {
            _tmp: tmp,
            oobo_home,
            repo,
        };
        sandbox.warm_oobo_home();
        sandbox.enable_project();
        sandbox
    }

    fn warm_oobo_home(&self) {
        let _ = Command::new(oobo_binary())
            .arg("--version")
            .current_dir(&self.repo)
            .env("OOBO_HOME", self.oobo_home.path())
            .env("OOBO_TEST", "1")
            .env("NO_COLOR", "1")
            .output()
            .expect("warm OOBO_HOME");
    }

    fn enable_project(&self) {
        let output = Command::new(oobo_binary())
            .args(["enable", "--agent"])
            .current_dir(&self.repo)
            .env("OOBO_HOME", self.oobo_home.path())
            .env("OOBO_TEST", "1")
            .env("NO_COLOR", "1")
            .output()
            .expect("enable test project");
        assert!(
            output.status.success(),
            "enable test project failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn git(repo: &Path, args: &[&str]) {
    let output = run_git_capture(repo, args);
    assert!(
        output.status.success(),
        "git {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_git_capture(repo: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} failed to spawn: {e}"))
}
