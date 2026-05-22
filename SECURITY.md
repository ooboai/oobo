# Security Policy

We take security seriously. We appreciate your efforts to responsibly disclose vulnerabilities and will make every effort to acknowledge your contributions.

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

Instead, please send security-related reports to **[security@oobo.ai](mailto:security@oobo.ai)** or use [GitHub Security Advisories](https://github.com/ooboai/oobo/security/advisories/new).

### What to include

- A clear description of the vulnerability
- Impact assessment — what an attacker could achieve
- Steps to reproduce
- Affected versions (if known)
- Suggested fix (optional)

### What to expect

- **Acknowledgment** — we will acknowledge receipt within 2 business days
- **Assessment** — initial assessment within 5 business days
- **Resolution** — we aim to resolve critical vulnerabilities within 90 days
- **Confidentiality** — all reports are kept confidential

## Supported Versions

We recommend always running the latest version of oobo. Security fixes are applied to the latest release only.

## Scope

This policy applies to:

- The oobo CLI binary
- Official oobo GitHub repositories
- The install script at `https://oobo.ai/install.sh`

### Out of scope

- Issues in third-party dependencies (please report these upstream)
- Denial of service requiring local access
- Issues that cannot be exploited without direct access to the user's machine

## Security Design

- **Read-only** — oobo reads local AI tool data but never writes to it
- **Local by default** — anchor metadata is pushed only to your existing git remote (alongside your code) via the pre-push hook. The optional remote search/delta API requires a separate API key
- **No telemetry** — oobo does not phone home or collect usage data
- **Secret redaction** — session content is scrubbed with [gitleaks](https://github.com/gitleaks/gitleaks) patterns before any sharing or sync
- **Config protection** — files containing API keys are automatically set to `0600` permissions on Unix

Each tool integration reads only local session metadata (timestamps, model names, token counts) from well-known paths. oobo never accesses credentials, browsing history, or file contents outside of AI tool storage directories.

## Safe Harbor

We support safe harbor for security researchers who act in good faith and in accordance with this policy.
