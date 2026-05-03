# oobo CLI spec

This folder is the **behavioral contract** for the `oobo` CLI. Every user-visible command, flag, and output mode is specified here with:

1. A prose description of what the command is supposed to do.
2. A concrete example of the expected output.
3. The expected exit code.
4. Any side effects (files written, DB rows inserted, hooks installed, etc.).
5. Notable error cases.

It serves two purposes:

- **Docs.** If you want to know how a command behaves, read the spec for that command.
- **Tests.** Each numbered section is written so a test harness can parse it and assert real behavior against the examples.

## Files

| File | Covers |
|---|---|
| `00-global-flags.md` | `--agent`, `--json`, `--interactive`, `--version`, `--help`, auto-detection, filter flags (`-n`, `--since`, `--tool`) |
| `01-bare.md` | Bare `oobo` (no subcommand) — four-quadrant behavior |
| `02-anchors.md` | Bare `oobo` memory feed + `oobo anchor show <sha>` drill-down |
| `03-blame.md` | `oobo blame <file> [commit]` (strict superset of `git blame`) |
| `04-search.md` | `oobo search <query>` (local-first, remote when an API key is configured) |
| `05-enable-disable.md` | `oobo enable`, `oobo disable` — per-project tracking toggle |
| `06-alias.md` | `oobo alias` — removed, legacy hint only |
| `07-setup.md` | `oobo setup` + flags (onboarding, repair, reindex) |
| `08-settings.md` | `oobo settings [scope] [verb] <key> [value]` |
| `09-update.md` | `oobo update` (self-update, incl. hidden `--post-update`) |
| `10-git-passthrough.md` | Git passthrough — removed; unknown commands → clap errors |
| `11-legacy-hints.md` | Removed 0.1.x commands and their hint messages (incl. `anchors`, `alias`) |
| `12-hooks.md` | Hidden `oobo hooks …` — agent/post-commit/pre-push/post-merge/post-rewrite |
| `13-env-vars.md` | Environment variables (`OOBO_HOME`, `NO_COLOR`, agent env, internal markers) |
| `14-turns-from.md` | `oobo goto` / `oobo back` — time travel between turns and commits |

## Conventions

### Entry format

Every invocation follows this block shape:

    ### INVOCATION
    `oobo some command --some-flag`

    **Context:** inside / outside repo, TTY / non-TTY, env vars set, etc.

    **Behavior:** prose describing what should happen.

    **Example output:**
    ```
    literal expected output (or pattern when output is non-deterministic)
    ```

    **Exit code:** `0` / `1` / `2`.

    **Side effects:** files written, DB rows inserted, etc. (or "none").

    **Error cases:**
    - ...

### Placeholders

- `<sha>`, `<file>`, `<key>`, `<value>` — required positional args.
- `[optional]` — optional args.
- `$REPO`, `$CWD` — stand-ins for paths that vary by environment.
- `{timestamp}`, `{uuid}`, `{hash}` — non-deterministic values the harness should accept as any matching token.

### Glossary

- **TTY.** stdout (and stdin) is attached to a terminal. Detected via `isatty`.
- **Agent env.** Any of `CURSOR_AGENT`, `CLAUDECODE`, `AIDER`, `CONTINUE_*`, `AICOMMITS` is set.
- **Agent mode.** `--agent` flag is active (explicit or auto-detected via non-TTY + agent env).
- **Pretty mode.** TTY output with colors/borders/TUI. Default when none of `--agent` / `--json` is set.
- **JSON mode.** `--json` flag is active. Full structured data.
- **Enabled project.** Project has `.oobo/config` and `[project].enabled` is not `false`.
- **Reserved verbs.** `anchor`, `anchors`, `search`, `enable`, `disable`, `setup`, `settings`, `update`, `hooks` (hidden). Anything else at position 1 is a clap error (or a legacy hint if it matches the hint table).

### Completeness check

Every user-visible and internal command in the 1.0 surface is specced in this folder:

- User-visible: `anchors`, `anchor` (show, blame, from), `search`, `enable`, `disable`, `setup`, `settings`, `update`.
- Hidden: `hooks` (agent / post-commit / pre-push / post-merge / post-rewrite), `update --post-update`.
- Bare `oobo` (the primary feed), legacy hints, global flags, environment variables.

Anything missing is a bug in the spec. Open an issue or patch the relevant file.

### Running the spec

The executable harness lives at `tests/cli_spec_harness.rs`:

    cargo test --test cli_spec_harness

The harness parses `### Invocation` blocks, checks the documented command footprint, verifies top-level help does not grow new public commands, and smoke-runs safe concrete invocations in fresh temp git repos with isolated `OOBO_HOME` values. It is intentionally incremental: richer exit-code, stdout/stderr, and side-effect assertions should be added as individual spec cases become deterministic enough to execute directly.

### Output-mode invariant

For every view command (bare `oobo`, `oobo blame`, `search`):

- `oobo X --agent` MUST emit only plain ASCII, no ANSI escapes, no box-drawing characters, no prose sentences. Columns are space-separated and row-per-line.
- `oobo X --json` MUST emit valid JSON parseable by `jq '.'`. Top-level type matches the command's documented schema.
- `oobo X --agent --json` MUST fail with exit code 2 and error message `--agent and --json are mutually exclusive` on stderr.
- With stdout redirected to a file, `oobo X` (no flag) MUST behave like `oobo X --agent`.
