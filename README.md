[![License](https://img.shields.io/badge/License-Apache_2.0_or_MIT-blue)](LICENSE-APACHE)
[![Issues](https://img.shields.io/github/issues/NoCodeInc/oobo-git)](https://github.com/NoCodeInc/oobo-git/issues)
![GitHub Release](https://img.shields.io/github/v/release/NoCodeInc/oobo-git)

<h1 align="center">oobo-git</h1>

<h3 align="center">
  Know What Your AI Wrote.
</h3>

<p align="center">
  Open source git wrapper that captures AI coding context alongside your commits<br/>
  and sends events to <strong>any</strong> endpoint.
</p>

<p align="center">
  <a href="https://oobo.ai/docs">Documentation</a> ·
  <a href="https://github.com/NoCodeInc/oobo-git/issues/new?labels=bug&template=bug_report.md">Report Bug</a> ·
  <a href="https://github.com/NoCodeInc/oobo-git/issues/new?labels=enhancement&template=feature_request.md">Request Feature</a>
</p>

---

## What is oobo?

`oobo` is a transparent `git` wrapper. Every git command passes through unchanged, but on write operations it automatically captures context about your AI coding sessions and sends structured events to any HTTP endpoint you configure.

- **Transparent** — use it exactly like `git`. Nothing changes about your workflow.
- **Endpoint-agnostic** — send events to your own server, a data warehouse, or the Oobo dashboard.
- **AI-context aware** — automatically detects and reads sessions from your AI coding tools. Purely local reads, no cloud sync.
- **Single binary** — compiled Rust, zero runtime dependencies.

### Supported tools

Cursor, Claude Code, Windsurf, Aider, Continue.dev, GitHub Copilot Chat, Zed, Trae, and OpenAI Codex CLI.

All tools are enabled by default. Toggle any of them with `oobo setup`.

---

## Installation

```bash
curl -fsSL https://oobo.ai/oobo-git/install.sh | bash
```

Or download a binary from [Releases](https://github.com/NoCodeInc/oobo-git/releases) and place it in your PATH.

**Supported platforms:** macOS (Apple Silicon or Intel), Linux (x86_64 or ARM64).

---

## Quick Start

```bash
# 1. Run the setup wizard
oobo setup

# 2. Use oobo anywhere you'd use git
oobo commit -m "fix auth middleware"
oobo push origin main

# 3. Browse your AI sessions
oobo sessions
```

That's it. Write operations capture AI context and fire an event to your configured endpoint. Read operations pass straight through to git with zero overhead.

---

## Usage

### As a git wrapper

```bash
oobo commit -m "fix auth middleware"
oobo push origin main
oobo merge feature-branch
oobo status          # passes through to git
oobo log --oneline   # passes through to git
```

### Browse AI sessions

```bash
oobo sessions                                    # interactive TUI list
oobo sessions --all                              # all projects
oobo sessions show 2c97                          # scrollable conversation viewer
oobo sessions show 2c97 --json                   # JSON output
oobo sessions export 2c97 --format md --out chat.md
```

### Other commands

```bash
oobo setup           # interactive configuration wizard
oobo dash            # show status and connection info
oobo ship            # send AI context to endpoint now
oobo alias install   # alias git→oobo in your shell
oobo alias uninstall # remove the alias
oobo --help
```

---

## Configuration

Run `oobo setup` for an interactive wizard, or edit `~/.oobo/config.toml` directly:

```toml
[server]
url = "https://your-endpoint.example.com"
api_key = "sk_..."

[git]
real_git_path = "/usr/bin/git"   # auto-detected
alias_enabled = false
```

Each AI tool can be toggled individually:

```toml
[cursor]
enabled = true

[claude]
enabled = false
```

All tools are enabled by default. The full list of tool sections: `cursor`, `claude`, `windsurf`, `aider`, `continue_dev`, `copilot`, `zed`, `trae`, `codex`.

---

## Contributing

oobo-git is open source under the [Apache License 2.0](LICENSE-APACHE) and [MIT License](LICENSE-MIT), and is the [copyright of its contributors](NOTICE). If you would like to contribute, please read the [contributing guide](CONTRIBUTING.md) to get started.

---

## License

Licensed under either of:

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project by you shall be dual licensed as above, without any additional terms or conditions.
