//! End-to-end test for the M4 subagent inference pipeline.
//!
//! Proves the complete stack works on a realistic Claude-style
//! transcript fixture:
//!
//! 1. Seed two JSONL files on disk — a parent transcript with a
//!    Task-tool invocation and an unrelated (orphan) subagent
//!    transcript with a template-preamble first user message.
//! 2. Ingest them with [`ClaudeTurnTap`] as two independent sessions
//!    (i.e. the FilesystemConvention explicit-link path is
//!    **not** used — this exercises only the heuristic path).
//! 3. Run [`infer_subagents_for_project`].
//! 4. Assert the parent_session_id / subagent_kind were written
//!    onto the child's sessions row, and an audit row exists.

use std::io::Write;
use std::path::Path;

use oobo::attribution::inference_runner::infer_subagents_for_project;
use oobo::core::turn::Turn;
use oobo::db::Db;
use oobo::taps::claude::ClaudeTurnTap;
use oobo::taps::{TapArtifact, TurnTap};

fn write_jsonl(path: &Path, lines: &[&str]) {
    let mut f = std::fs::File::create(path).expect("create jsonl fixture");
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
    f.flush().unwrap();
}

fn seed_project(db: &Db, pid: &str) {
    db.conn
        .execute(
            "INSERT OR IGNORE INTO projects (id, path, name, discovered_at, last_seen_at) \
             VALUES (?1, ?1, ?1, 0, 0)",
            [pid],
        )
        .unwrap();
}

fn seed_session(db: &Db, pid: &str, sid: &str) {
    db.conn
        .execute(
            "INSERT OR IGNORE INTO sessions (id, source, project_id, indexed_at) \
             VALUES (?1, 'claude', ?2, 0)",
            [sid, pid],
        )
        .unwrap();
}

#[test]
fn end_to_end_taps_then_inference_links_orphan_subagent() {
    let mut db = Db::open_in_memory().unwrap();
    let pid = "r:acme/repo";
    seed_project(&db, pid);
    seed_session(&db, pid, "parent-sess");
    seed_session(&db, pid, "orphan-sess");

    let dir = tempfile::tempdir().unwrap();
    let parent_path = dir.path().join("parent.jsonl");
    let orphan_path = dir.path().join("orphan.jsonl");

    write_jsonl(
        &parent_path,
        &[
            r#"{"type":"user","message":{"content":"Find me the thing"},"timestamp":"2026-04-22T10:00:00Z"}"#,
            // Key: an assistant turn that fires the Task tool at T=10:00:05.
            r#"{"type":"assistant","message":{"model":"claude-opus-4-5","usage":{"input_tokens":50,"output_tokens":100},"content":[{"type":"text","text":"I'll launch a subagent"},{"type":"tool_use","name":"Task","id":"tu_task_1","input":{"subagent_type":"explore","prompt":"Find the thing"}}]},"timestamp":"2026-04-22T10:00:05Z"}"#,
        ],
    );

    write_jsonl(
        &orphan_path,
        &[
            // Starts 2s after the parent's Task call. Template preamble present.
            r#"{"type":"user","message":{"content":"You are a task-focused agent. Find the thing described above."},"timestamp":"2026-04-22T10:00:07Z"}"#,
            r#"{"type":"assistant","message":{"usage":{"output_tokens":40},"content":[{"type":"text","text":"Found it at path/to/thing"}]},"timestamp":"2026-04-22T10:00:30Z"}"#,
        ],
    );

    db.with_turn_sink(Some(pid.into()), |sink| {
        ClaudeTurnTap
            .ingest_session("parent-sess", TapArtifact::File(&parent_path), sink)
            .unwrap();
        ClaudeTurnTap
            .ingest_session("orphan-sess", TapArtifact::File(&orphan_path), sink)
            .unwrap();
    })
    .unwrap();

    let n_turns: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get::<_, i64>(0))
        .unwrap();
    assert_eq!(n_turns, 4, "2 turns per session × 2 sessions");

    let tool_name: Option<String> = db
        .conn
        .query_row(
            "SELECT tool_names FROM turns WHERE session_id='parent-sess' AND tool_names IS NOT NULL",
            [],
            |r| r.get::<_, Option<String>>(0),
        )
        .unwrap();
    assert_eq!(
        tool_name.as_deref(),
        Some("Task"),
        "tap must have emitted the Task tool_name for the inference engine to pick up"
    );

    let report = infer_subagents_for_project(&db, pid).unwrap();
    assert_eq!(report.proposed, 1);
    assert_eq!(report.applied, 1, "temporal + template preamble → apply");

    // Sessions row reflects the inferred link.
    let (parent_sid, subagent_kind): (Option<String>, Option<String>) = db
        .conn
        .query_row(
            "SELECT parent_session_id, subagent_kind \
             FROM sessions WHERE id='orphan-sess' AND source='claude'",
            [],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .unwrap();
    assert_eq!(parent_sid.as_deref(), Some("parent-sess"));
    assert_eq!(subagent_kind.as_deref(), Some("task"));

    // Audit row was written with a matching score above threshold.
    let (score, signals): (f64, String) = db
        .conn
        .query_row(
            "SELECT score, signals_json \
             FROM subagent_inferences WHERE child_session_id='orphan-sess'",
            [],
            |r| Ok((r.get::<_, f64>(0)?, r.get::<_, String>(1)?)),
        )
        .unwrap();
    assert!(score >= 0.6, "score above APPLY_THRESHOLD; got {score}");
    assert!(
        signals.contains("task_tool_temporal") && signals.contains("template_preamble"),
        "both signals must have fired; got: {signals}"
    );

    // Determinism: re-running must produce the same final state
    // (audit row count grows by one because decided_at changes, but
    // the applied link is stable).
    let report2 = infer_subagents_for_project(&db, pid).unwrap();
    // Now orphan-sess has a parent, so it's no longer in the pool.
    assert_eq!(report2.orphans_considered, 1, "only parent-sess remains orphan");
    assert_eq!(report2.applied, 0, "no new links to apply");

    let _ = Turn::deterministic_id("claude", "x", 0); // sanity: types exported
}
