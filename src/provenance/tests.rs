use super::*;
use std::collections::HashMap;

/// In-memory blob source: hash → content. Keeps the engine tests pure.
struct MemBlobs(HashMap<String, String>);

impl MemBlobs {
    fn new() -> Self {
        MemBlobs(HashMap::new())
    }
    fn put(&mut self, name: &str, content: &str) -> String {
        self.0.insert(name.to_string(), content.to_string());
        name.to_string()
    }
}

impl BlobSource for MemBlobs {
    fn read(&self, blob: &str) -> Option<String> {
        if is_null_blob(blob) {
            return Some(String::new());
        }
        self.0.get(blob).cloned()
    }
}

fn edit(session: &str, seq: i64, pre: &str, post: &str) -> EditInput {
    EditInput {
        session_id: session.to_string(),
        agent: "claude".to_string(),
        turn_index: Some(seq),
        seq,
        timestamp_us: seq * 1_000_000,
        pre_blob: pre.to_string(),
        post_blob: post.to_string(),
        tool_name: Some("Edit".to_string()),
        trigger: Trigger::AgentAutonomous,
    }
}

// ── diff_hunks: the metadata-patching foundation ────────────────────────

#[test]
fn diff_hunks_pure_insert() {
    let hunks = diff_hunks("a\nb\n", "a\nx\ny\nb\n");
    assert_eq!(
        hunks,
        vec![Hunk {
            old_start: 1,
            old_count: 0,
            new_start: 2,
            new_count: 2
        }]
    );
}

#[test]
fn diff_hunks_pure_delete() {
    let hunks = diff_hunks("a\nx\nb\n", "a\nb\n");
    assert_eq!(
        hunks,
        vec![Hunk {
            old_start: 2,
            old_count: 1,
            new_start: 1,
            new_count: 0
        }]
    );
}

#[test]
fn diff_hunks_replace() {
    let hunks = diff_hunks("a\nold\nb\n", "a\nnew\nb\n");
    assert_eq!(
        hunks,
        vec![Hunk {
            old_start: 2,
            old_count: 1,
            new_start: 2,
            new_count: 1
        }]
    );
}

#[test]
fn diff_hunks_from_empty_and_to_empty() {
    assert_eq!(
        diff_hunks("", "a\nb\n"),
        vec![Hunk {
            old_start: 0,
            old_count: 0,
            new_start: 1,
            new_count: 2
        }]
    );
    assert_eq!(
        diff_hunks("a\nb\n", ""),
        vec![Hunk {
            old_start: 1,
            old_count: 2,
            new_start: 0,
            new_count: 0
        }]
    );
    assert!(diff_hunks("", "").is_empty());
    assert!(diff_hunks("same\n", "same\n").is_empty());
}

#[test]
fn diff_hunks_multiple_regions() {
    let a = "1\n2\n3\n4\n5\n";
    let b = "1\nX\n3\n4\n5\nY\n";
    let hunks = diff_hunks(a, b);
    assert_eq!(hunks.len(), 2);
    assert_eq!(
        hunks[0],
        Hunk {
            old_start: 2,
            old_count: 1,
            new_start: 2,
            new_count: 1
        }
    );
    assert_eq!(
        hunks[1],
        Hunk {
            old_start: 5,
            old_count: 0,
            new_start: 6,
            new_count: 1
        }
    );
}

/// Round-trip property: applying hunks to `a`'s line vector must produce
/// a vector with exactly `b`'s line count — for a pile of tricky pairs.
#[test]
fn apply_diff_preserves_line_count_invariant() {
    let cases = [
        ("a\nb\nc\n", "a\nc\n"),
        ("", "x\n"),
        ("x\n", ""),
        ("a\nb\n", "b\na\n"),
        ("1\n2\n3\n4\n", "4\n3\n2\n1\n"),
        ("x\nx\nx\n", "x\nx\n"),
        ("fn a() {}\n\n}\n", "fn a() {}\nfn b() {}\n\n}\n"),
        ("a\nb\nc\nd\ne\n", "e\nd\nc\nb\na\n"),
    ];
    for (a, b) in cases {
        let attr = vec![LineOrigin::Baseline; a.lines().count()];
        let mut clob = HashMap::new();
        let (new_attr, _, _) =
            apply_diff(a, b, &attr, LineOrigin::Edit { step: 0 }, &mut clob, None);
        assert_eq!(
            new_attr.len(),
            b.lines().count(),
            "line-count invariant broken for {a:?} -> {b:?}"
        );
    }
}

// ── the replay engine ───────────────────────────────────────────────────

#[test]
fn single_edit_attributes_added_lines() {
    let mut blobs = MemBlobs::new();
    let base = blobs.put("base", "line1\nline2\n");
    let post = blobs.put("post", "line1\nadded\nline2\n");

    let p = compute_file_provenance(
        &blobs,
        "c1",
        "f.rs",
        &base,
        &post,
        vec![edit("s1", 1, "base", "post")],
    );

    assert_eq!(p.lines.len(), 3);
    assert_eq!(p.lines[0], LineOrigin::Baseline);
    assert_eq!(p.lines[1], LineOrigin::Edit { step: 0 });
    assert_eq!(p.lines[2], LineOrigin::Baseline);
    assert!(p.steps[0].linked);
    assert_eq!(p.steps[0].lines_added, 1);
    assert_eq!(p.steps[0].lines_surviving, 1);
    assert!(p.gaps.is_empty());
    assert!(p.clobbers.is_empty());
}

/// The two-session interleave from the plan's "done when": A and B edit
/// the same file; every line lands on its true author and the override
/// chain is visible.
#[test]
fn two_session_interleave_attributes_each_line_to_true_author() {
    let mut blobs = MemBlobs::new();
    let base = blobs.put("v0", "shared\n");
    blobs.put("v1", "shared\nfrom-a\n");
    blobs.put("v2", "shared\nfrom-a\nfrom-b\n");
    let v3 = blobs.put("v3", "shared\nfrom-a2\nfrom-a\nfrom-b\n");

    let edits = vec![
        edit("session-a", 1, "v0", "v1"),
        edit("session-b", 2, "v1", "v2"),
        edit("session-a", 3, "v2", "v3"),
    ];
    let p = compute_file_provenance(&blobs, "c1", "f.rs", &base, &v3, edits);

    assert_eq!(p.lines.len(), 4);
    assert_eq!(p.lines[0], LineOrigin::Baseline);
    assert_eq!(p.lines[1], LineOrigin::Edit { step: 2 }); // from-a2 (A again)
    assert_eq!(p.lines[2], LineOrigin::Edit { step: 0 }); // from-a
    assert_eq!(p.lines[3], LineOrigin::Edit { step: 1 }); // from-b

    assert_eq!(p.steps[0].edit.session_id, "session-a");
    assert_eq!(p.steps[1].edit.session_id, "session-b");
    assert_eq!(p.steps[2].edit.session_id, "session-a");
    assert!(p.steps.iter().all(|s| s.linked), "fully chained history");
    assert!(p.gaps.is_empty());
}

/// A wrote, B overwrote: both stay in the chain; A gets a clobber record.
#[test]
fn clobber_keeps_both_and_records_loss() {
    let mut blobs = MemBlobs::new();
    let base = blobs.put("v0", "keep\n");
    blobs.put("v1", "keep\nA-version\n");
    let v2 = blobs.put("v2", "keep\nB-version\n");

    let edits = vec![edit("a", 1, "v0", "v1"), edit("b", 2, "v1", "v2")];
    let p = compute_file_provenance(&blobs, "c1", "f.rs", &base, &v2, edits);

    // Final line belongs to B.
    assert_eq!(p.lines[1], LineOrigin::Edit { step: 1 });
    // A's step survives in the record with zero surviving lines.
    assert_eq!(p.steps[0].lines_added, 1);
    assert_eq!(p.steps[0].lines_surviving, 0);
    // And the clobber is explicit: A's work overwritten by step 1.
    assert_eq!(
        p.clobbers,
        vec![ClobberRecord {
            victim_step: 0,
            by_step: Some(1),
            lines_lost: 1
        }]
    );
}

/// Hand-edit between turns: the file changed without capture. The line
/// is flagged Uncaptured, bounded by the steps around the window.
#[test]
fn hand_edit_between_turns_is_uncaptured_with_bounds() {
    let mut blobs = MemBlobs::new();
    let base = blobs.put("v0", "one\n");
    blobs.put("v1", "one\nagent-line\n");
    // Hand edit (no capture): "human-line" appears.
    blobs.put("v2", "one\nagent-line\nhuman-line\n");
    let v3 = blobs.put("v3", "one\nagent-line\nhuman-line\nagent-line-2\n");

    let edits = vec![
        edit("s1", 1, "v0", "v1"),
        edit("s1", 2, "v2", "v3"), // pre v2 ≠ previous post v1 → gap
    ];
    let p = compute_file_provenance(&blobs, "c1", "f.rs", &base, &v3, edits);

    assert_eq!(p.lines[0], LineOrigin::Baseline);
    assert_eq!(p.lines[1], LineOrigin::Edit { step: 0 });
    assert_eq!(p.lines[2], LineOrigin::Uncaptured { gap: 0 });
    assert_eq!(p.lines[3], LineOrigin::Edit { step: 1 });

    assert_eq!(p.gaps.len(), 1);
    assert_eq!(p.gaps[0].after_step, Some(0), "bounded below by step 0");
    assert_eq!(p.gaps[0].before_step, Some(1), "bounded above by step 1");
    assert_eq!(p.gaps[0].lines_added, 1);
    assert!(!p.steps[1].linked, "second step is chain-disconnected");
}

/// Trailing hand edit after the last turn (or partial staging): the
/// delta between last capture and the committed blob is a gap with
/// `before_step = None`.
#[test]
fn trailing_uncaptured_window_after_last_turn() {
    let mut blobs = MemBlobs::new();
    let base = blobs.put("v0", "one\n");
    blobs.put("v1", "one\nagent\n");
    let committed = blobs.put("vc", "one\nagent\nhand-after\n");

    let p = compute_file_provenance(
        &blobs,
        "c1",
        "f.rs",
        &base,
        &committed,
        vec![edit("s1", 1, "v0", "v1")],
    );

    assert_eq!(p.lines[2], LineOrigin::Uncaptured { gap: 0 });
    assert_eq!(p.gaps[0].after_step, Some(0));
    assert_eq!(p.gaps[0].before_step, None, "trailing window");
}

/// Partial staging: agent added A and B, only A was committed. B's step
/// loses a line to the trailing window; A's line is attributed.
#[test]
fn partial_staging_attribution_and_loss() {
    let mut blobs = MemBlobs::new();
    let base = blobs.put("v0", "base\n");
    blobs.put("v1", "base\nA\nB\n");
    let committed = blobs.put("vc", "base\nA\n");

    let p = compute_file_provenance(
        &blobs,
        "c1",
        "f.rs",
        &base,
        &committed,
        vec![edit("s1", 1, "v0", "v1")],
    );

    assert_eq!(p.lines[1], LineOrigin::Edit { step: 0 });
    assert_eq!(p.steps[0].lines_added, 2);
    assert_eq!(p.steps[0].lines_surviving, 1);
    // The un-staged line was lost to the (uncaptured) staging delta.
    assert_eq!(
        p.clobbers,
        vec![ClobberRecord {
            victim_step: 0,
            by_step: None,
            lines_lost: 1
        }]
    );
}

/// Human reverts the agent's work entirely: clobber with by_step=None,
/// zero survivors, file back to baseline.
#[test]
fn human_revert_of_agent_work() {
    let mut blobs = MemBlobs::new();
    let base = blobs.put("v0", "base\n");
    blobs.put("v1", "base\nagent\n");
    let committed = "v0"; // back to baseline

    let p = compute_file_provenance(
        &blobs,
        "c1",
        "f.rs",
        &base,
        committed,
        vec![edit("s1", 1, "v0", "v1")],
    );

    assert_eq!(p.lines.len(), 1);
    assert_eq!(p.lines[0], LineOrigin::Baseline);
    assert_eq!(p.steps[0].lines_surviving, 0);
    assert_eq!(p.clobbers[0].by_step, None);
}

/// New file created by the agent from nothing.
#[test]
fn new_file_from_null_blob() {
    let mut blobs = MemBlobs::new();
    let post = blobs.put("v1", "a\nb\nc\n");

    let p = compute_file_provenance(
        &blobs,
        "c1",
        "new.rs",
        "", // no baseline
        &post,
        vec![edit("s1", 1, "", "v1")],
    );

    assert_eq!(p.lines.len(), 3);
    assert!(p.lines.iter().all(|l| *l == LineOrigin::Edit { step: 0 }));
    assert!(p.steps[0].linked, "null pre == empty baseline → linked");
}

/// No captured edits at all: every added line is uncaptured (one
/// trailing window), baseline lines stay baseline.
#[test]
fn no_capture_at_all_is_one_uncaptured_window() {
    let mut blobs = MemBlobs::new();
    let base = blobs.put("v0", "old\n");
    let committed = blobs.put("vc", "old\nnew\n");

    let p = compute_file_provenance(&blobs, "c1", "f.rs", &base, &committed, vec![]);

    assert_eq!(p.lines[0], LineOrigin::Baseline);
    assert_eq!(p.lines[1], LineOrigin::Uncaptured { gap: 0 });
    assert_eq!(p.gaps.len(), 1);
    assert_eq!(p.gaps[0].after_step, None);
    assert_eq!(p.gaps[0].before_step, None);
}

/// Causal order comes from content, not wall clocks: events arriving
/// out of timestamp order still replay; the disconnected one is marked.
#[test]
fn out_of_order_events_are_sorted_by_time_then_seq() {
    let mut blobs = MemBlobs::new();
    let base = blobs.put("v0", "x\n");
    blobs.put("v1", "x\nfirst\n");
    let v2 = blobs.put("v2", "x\nfirst\nsecond\n");

    // Pass them in reverse order; sort must restore the chain.
    let edits = vec![edit("s1", 2, "v1", "v2"), edit("s1", 1, "v0", "v1")];
    let p = compute_file_provenance(&blobs, "c1", "f.rs", &base, &v2, edits);

    assert!(p.steps.iter().all(|s| s.linked));
    assert_eq!(p.lines[1], LineOrigin::Edit { step: 0 });
    assert_eq!(p.lines[2], LineOrigin::Edit { step: 1 });
}

/// No-op events (pre == post, e.g. a formatter that changed nothing)
/// are dropped from the chain.
#[test]
fn noop_events_are_dropped() {
    let mut blobs = MemBlobs::new();
    let base = blobs.put("v0", "x\n");
    let v1 = blobs.put("v1", "x\ny\n");

    let edits = vec![edit("s1", 1, "v0", "v0"), edit("s1", 2, "v0", "v1")];
    let p = compute_file_provenance(&blobs, "c1", "f.rs", &base, &v1, edits);

    assert_eq!(p.steps.len(), 1);
    assert_eq!(p.lines[1], LineOrigin::Edit { step: 0 });
}

/// line lookup helpers.
#[test]
fn origin_and_step_lookup_by_lineno() {
    let mut blobs = MemBlobs::new();
    let base = blobs.put("v0", "one\n");
    let v1 = blobs.put("v1", "one\ntwo\n");

    let p = compute_file_provenance(
        &blobs,
        "c1",
        "f.rs",
        &base,
        &v1,
        vec![edit("s9", 1, "v0", "v1")],
    );

    assert_eq!(p.origin_of(1), Some(&LineOrigin::Baseline));
    assert!(p.step_for_line(1).is_none());
    assert_eq!(p.step_for_line(2).unwrap().edit.session_id, "s9");
    assert!(p.origin_of(99).is_none());
}

/// Serde round-trip: the cache stores this structure as JSON.
#[test]
fn provenance_serde_roundtrip() {
    let mut blobs = MemBlobs::new();
    let base = blobs.put("v0", "a\n");
    let v1 = blobs.put("v1", "a\nb\n");
    let p = compute_file_provenance(
        &blobs,
        "c1",
        "f.rs",
        &base,
        &v1,
        vec![edit("s1", 1, "v0", "v1")],
    );

    let json = serde_json::to_string(&p).unwrap();
    let back: FileProvenance = serde_json::from_str(&json).unwrap();
    assert_eq!(back.lines, p.lines);
    assert_eq!(back.steps.len(), p.steps.len());
    assert_eq!(back.clobbers, p.clobbers);
}
