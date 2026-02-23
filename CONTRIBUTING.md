# Contributing to oobo-git

Thanks for your interest in contributing! We welcome contributions from the community.

> If you like the project but don't have time to contribute code, there are other ways to help:
>
> - Star the project
> - Share it with colleagues who use AI coding tools
> - Report bugs or request features via [issues](https://github.com/NoCodeInc/oobo-git/issues)
> - Improve the documentation

## Code of Conduct

This project follows the [Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md). By participating, you are expected to uphold this code.

## Ways to Contribute

### Report Bugs

If you encounter a bug, [open an issue](https://github.com/NoCodeInc/oobo-git/issues/new?labels=bug) with:

- What you expected to happen
- What actually happened
- Steps to reproduce
- Your OS and oobo version (`oobo --version`)

### Suggest Features

Have an idea? [Open a feature request](https://github.com/NoCodeInc/oobo-git/issues/new?labels=enhancement). Before creating a new issue, check if one already exists.

### Contribute Code

1. **Fork** the repository
2. **Create** a feature branch (`git checkout -b feature/my-feature`)
3. **Make** your changes
4. **Test** your changes (`cargo test`)
5. **Lint** your changes (`cargo clippy --all-targets && cargo fmt --check`)
6. **Commit** with a descriptive message
7. **Push** to your fork and open a pull request

### What Makes a Good Pull Request

- **Focused** — one feature or fix per PR
- **Tested** — add or update tests for your changes
- **Clean** — no unrelated changes, passes clippy and fmt
- **Documented** — update README if you add or change commands

## Development Setup

```bash
# Clone your fork
git clone https://github.com/YOUR_USERNAME/oobo-git.git
cd oobo-git

# Build
cargo build

# Run tests
cargo test

# Run lints
cargo clippy --all-targets
cargo fmt --check

# Run with arguments
cargo run -- sessions
cargo run -- dash
```

### Project Overview

oobo-git is a Rust binary that acts as a transparent git proxy. Key modules:

| Module | Purpose |
|--------|---------|
| `src/cli.rs` | Command-line argument parsing and routing |
| `src/git/` | Git proxy, write operation detection, context collection |
| `src/cursor/` | Cursor IDE local data extraction (SQLite, transcripts) |
| `src/claude/` | Claude Code local data extraction (JSONL sessions) |
| `src/vscode_fork.rs` | Shared VS Code fork extraction (Windsurf, Trae) |
| `src/windsurf.rs` | Windsurf / Cascade support |
| `src/trae.rs` | Trae IDE support |
| `src/aider.rs` | Aider chat history (markdown) |
| `src/continue_dev.rs` | Continue.dev session support |
| `src/copilot.rs` | GitHub Copilot Chat session support |
| `src/zed.rs` | Zed AI conversation support |
| `src/codex.rs` | OpenAI Codex CLI session support |
| `src/session.rs` | Multi-source session dispatch |
| `src/server/` | HTTP event delivery |
| `src/tui/` | Ratatui terminal UI for interactive commands |
| `src/config.rs` | Configuration file management |

### Running Tests

```bash
# All tests
cargo test

# A specific test
cargo test test_write_op_detection

# With output
cargo test -- --nocapture
```

### CI

Pull requests run through GitHub Actions:

- `cargo check` on Ubuntu and macOS
- `cargo test` on Ubuntu and macOS
- `cargo clippy --all-targets`
- `cargo fmt --check`
- Docker builds on Ubuntu, Debian, and Alpine

All checks must pass before merge.

## Review Process

1. A maintainer will review your PR within a few business days
2. We may ask for changes or suggest improvements
3. Once approved, a maintainer will merge your PR

We try to give you the opportunity to make changes yourself, but may make minor edits directly if it makes sense.

## License

By contributing to oobo-git, you agree that your contributions will be dual licensed under the [Apache License 2.0](LICENSE-APACHE) and [MIT License](LICENSE-MIT).
