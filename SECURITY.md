# Security Policy

## Reporting a Vulnerability

If you believe you have found a security vulnerability in oobo-git, please report it responsibly.

**Please do NOT report security vulnerabilities through public GitHub issues.**

Instead, please email us at: **security@oobo.ai**

You can also report vulnerabilities privately through [GitHub's security advisory feature](https://github.com/NoCodeInc/oobo-git/security/advisories/new).

Please include:

- Description of the vulnerability
- Steps to reproduce
- Impact assessment
- Any relevant proof-of-concept

We will acknowledge receipt within 2 business days and provide an initial assessment within 5 business days.

## Supported Versions

We accept vulnerability reports for the latest stable release of oobo-git.

## Data Handling

oobo-git reads data from local AI tool storage:

- **Cursor** — SQLite databases and transcript files in `~/Library/Application Support/Cursor/`
- **Claude Code** — session JSONL files under `~/.claude/`
- **Windsurf** — SQLite databases in `~/Library/Application Support/Windsurf/`
- **Trae** — SQLite databases in `~/Library/Application Support/Trae/`
- **Aider** — `.aider.chat.history.md` in project directories
- **Continue.dev** — session files under `~/.continue/`
- **GitHub Copilot Chat** — JSON session files in VS Code workspace storage
- **Zed** — conversation files in `~/Library/Application Support/Zed/`
- **OpenAI Codex CLI** — JSONL session logs in `~/.codex/sessions/`

All reads are read-only. oobo never writes to any AI tool's data, and never modifies git history.

Event payloads are sent to the endpoint you configure. The CLI does not phone home or send data to any endpoint unless you explicitly configure one.

## Safe Harbor

We support safe harbor for security researchers who act in good faith and in accordance with this policy.
