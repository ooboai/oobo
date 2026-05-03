# Environment variables

Variables that change oobo's behavior. Part of the public contract — tests, scripts, and integrations depend on them. Any change to one of these is a breaking change.

## User-facing

### `OOBO_HOME`

Override the base directory for oobo's local state.

- **Default:** `$HOME/.oobo`.
- **When set:** config, db, logs, skills, and backups all live under `$OOBO_HOME/` instead.
- **Precedence:** wins over every derived path (db, config, logs).
- **Use cases:** tests (every spec test sets this to a tempdir), dev sandboxes, ephemeral CI environments.

Example:
```bash
OOBO_HOME=/tmp/oobo-sandbox oobo setup --non-interactive
ls /tmp/oobo-sandbox/
# → config.toml  db/  logs/
```

Invariants:
- When unset, oobo uses `$HOME/.oobo`.
- When set but directory is not writable → exit `1` with `error: OOBO_HOME is not writable: <path>`.
- Must be an absolute path; relative paths are rejected with exit `2`.

---

### `NO_COLOR`

Standard cross-tool convention. When set (to any value, even empty string), oobo emits no ANSI color codes in pretty mode.

- **Default:** unset (colors on).
- Affects ALL commands.

Example:
```bash
NO_COLOR=1 oobo   # plain output, no colors
```

Invariants:
- `NO_COLOR=` (empty) disables colors (per the spec at `no-color.org`).
- Colors are additionally suppressed when stdout is not a TTY, independently of `NO_COLOR`.

---

### `OOBO_DEBUG`

Enable verbose debug logging on stderr and in log files.

- **Default:** unset (quiet).
- **Values:** `1` / `true` / `yes` enables; anything else (or unset) disables.

Example:
```bash
OOBO_DEBUG=1 oobo 2>oobo.log
```

When set:
- Stderr receives verbose per-operation traces (DB queries, git commands, network calls).
- `~/.oobo/logs/debug.log` (or `$OOBO_HOME/logs/debug.log`) is created and written to in parallel.

Invariants:
- Does NOT change stdout. Tests relying on stdout match aren't broken by this.
- Takes precedence over `RUST_LOG` if both are set for the oobo binary.

---

### `OOBO_SECRET_KEY`

Provide the API key for this process without writing it to disk.

- **Default:** unset.
- **When set:** overrides persisted default and project keys.
- **Auth:** the value is used for remote search in this process.
- **Use cases:** CI, ephemeral sandboxes, one-off remote search.

Example:
```bash
OOBO_SECRET_KEY=sk_... oobo search "auth" --remote
```

Invariants:
- Empty `OOBO_SECRET_KEY` is ignored.
- The value is never printed unmasked by `oobo settings`.
- The environment value wins over persisted default and project keys for the current process.

---

### `RUST_LOG`

Standard Rust `env_logger` / `tracing` convention. Accepted but `OOBO_DEBUG` is preferred for human use.

- **Default:** unset.
- **Values:** `error`, `warn`, `info`, `debug`, `trace`, or fine-grained: `oobo=debug,reqwest=info`.

Primarily useful for developers debugging a specific module.

---

## Agent detection

Any of these env vars, set to a non-empty value, implicitly forces `--agent` mode (see `00-global-flags.md`):

- `CURSOR_AGENT`
- `CLAUDECODE`
- `AIDER`
- `CONTINUE_SESSION` / `CONTINUE_IDE`
- `AICOMMITS`

Rationale: when oobo is invoked by an AI tool, token-efficient output is always the right default.

Invariants:
- Auto-detection is a soft nudge; explicit `--interactive` overrides it.
- Explicit `--json` also overrides (JSON wins over auto-agent).

---

## Internal (set by oobo itself)

These are NOT meant to be set manually; they're part of oobo's re-entry and test machinery.

### `OOBO_INTERCEPTED`

Marker that oobo sets when it shells out to `git` from within its own commit/push interceptor. The installed git hooks check for this and no-op if present, preventing infinite recursion.

- **Set by:** `crate::git::interceptor::on_write_op`, `crate::git::proxy::run_and_intercept`.
- **Read by:** `oobo hooks post-commit`, `oobo hooks pre-push`.

If a user manually sets this in their shell, oobo's hooks become no-ops — useful for reproducing bugs where hooks cause interference.

### `OOBO_SKIP_UPDATE_CHECK`

Suppresses the background update-check that runs occasionally. Used in tests and CI.

- **Default:** unset (check runs).
- **Values:** `1` / `true` disables.

### `OOBO_TEST`

Opt-in marker used by the test harness and by `tests/cli-spec/run.sh`. When set:
- Suppresses first-run banner.
- Suppresses any "did you know" prompts.
- Forces deterministic timestamps in agent output (optional; may be removed in favor of pattern matchers).

- **Default:** unset.
- **Values:** `1` enables.

---

## Examples of combined use in specs

### Isolated test invocation

```bash
OOBO_HOME=$(mktemp -d) OOBO_TEST=1 NO_COLOR=1 oobo settings set key sk_test
```

This is the canonical shape for every invocation the `tests/cli-spec/run.sh` harness will perform: isolated home, silent banners, no colors.

### Debugging a missed anchor

```bash
OOBO_DEBUG=1 oobo hooks post-commit 2>&1 | tee /tmp/oobo-commit.log
```

Shows the full trace of project resolution, session matching, and orphan-branch write.

### Forcing agent mode in an SSH session

```bash
CURSOR_AGENT=1 ssh user@host 'cd repo && oobo'
# The remote oobo emits --agent-style output despite having no local TTY hint.
```

---

## Invariants

- `OOBO_HOME` takes precedence over `$HOME`-derived defaults for EVERY file oobo writes.
- `NO_COLOR` and `--agent` both independently disable ANSI escapes.
- Agent env vars only auto-flip to `--agent`, never to `--json`.
- `OOBO_INTERCEPTED` set in the user's environment disables all write-path hooks — NEVER recommend setting it except for debugging.
- Changing any listed env var's name or semantics in a non-major release is a breaking change.
