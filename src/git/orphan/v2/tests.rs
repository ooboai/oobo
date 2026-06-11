use super::*;

fn init_repo() -> Option<(tempfile::TempDir, String)> {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_str().unwrap().to_string();
    let init = std::process::Command::new("git")
        .args(["init", &repo])
        .output();
    if init.is_err() || !init.unwrap().status.success() {
        return None;
    }
    for args in [
        &["config", "user.name", "Test"][..],
        &["config", "user.email", "test@test.com"][..],
    ] {
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output();
    }
    let _ = std::process::Command::new("git")
        .args(["-C", &repo, "commit", "--allow-empty", "-m", "init"])
        .output();
    Some((tmp, repo))
}

fn make_anchor(sha: &str, committed_at: i64) -> crate::core::anchor::Anchor {
    crate::core::anchor::Anchor {
        anchor_schema_version: crate::core::anchor::ANCHOR_SCHEMA_VERSION,
        oobo_version: "test".into(),
        commit_hash: sha.into(),
        branch: "main".into(),
        author: "Test <t@t>".into(),
        author_type: crate::core::anchor::AuthorType::Assisted,
        contributors: Vec::new(),
        committed_at,
        message: "test commit".into(),
        files_changed: vec!["src/a.rs".into()],
        added: 1,
        deleted: 0,
        file_changes: Vec::new(),
        ai_added: 1,
        ai_deleted: 0,
        human_added: 0,
        human_deleted: 0,
        ai_percentage: Some(100.0),
        session_ids: Vec::new(),
        summary: None,
        intent: None,
        reasoning: None,
        transparency_mode: crate::core::anchor::TransparencyMode::Off,
        file_interactions: None,
        turns: Vec::new(),
    }
}

fn make_session(uid: &str, updated_at: i64) -> SessionRecord {
    SessionRecord {
        schema_version: V2_SCHEMA_VERSION,
        session_uid: uid.into(),
        native_session_ids: vec!["native-1".into()],
        tool: "claude".into(),
        model: Some("opus".into()),
        home_location: None,
        origin_repo_id: Some("r:github.com/acme/x".into()),
        repos_touched: vec!["r:github.com/acme/x".into()],
        lineage: SessionLineage::default(),
        turn_count: 1,
        title: None,
        started_at: updated_at - 100,
        updated_at,
        ended_at: None,
    }
}

// ── repo_key ────────────────────────────────────────────────────────────

#[test]
fn repo_key_is_tree_safe_and_collision_distinct() {
    let a = repo_key("r:github.com/acme/widget");
    let b = repo_key("r:github.com/acme-widget"); // would slug-collide without hash
    assert_ne!(a, b, "slug collisions must be disambiguated by hash");
    for key in [&a, &b] {
        assert!(
            key.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_'),
            "repo key must be tree-safe: {key}"
        );
    }
    assert_eq!(a, repo_key("r:github.com/acme/widget"), "deterministic");
}

// ── anchors ─────────────────────────────────────────────────────────────

#[test]
fn anchor_roundtrip_and_time_index() {
    let Some((_tmp, repo)) = init_repo() else {
        return;
    };
    let repo_id = "r:github.com/acme/x";

    let sha_old = "aa11000000000000000000000000000000000001";
    let sha_new = "bb22000000000000000000000000000000000002";
    for (sha, ts) in [(sha_old, 1000), (sha_new, 2000)] {
        let record = AnchorRecord {
            anchor: make_anchor(sha, ts),
            session_refs: vec![SessionRef {
                session_uid: "suid-1".into(),
                home_location: Some("r:github.com/acme/x".into()),
                turn_uids: vec!["tuid-1".into()],
            }],
            session_links: Vec::new(),
            coverage: Some(CoverageManifest {
                tools: vec!["claude".into()],
                hook_events_seen: vec!["stop".into()],
                capture_gap_files: Vec::new(),
                recorded_at: ts,
            }),
        };
        write_anchor(&repo, repo_id, &record, Some("{\"events\":[]}")).unwrap();
    }

    // O(1) lookup by sha.
    let back = read_anchor(&repo, repo_id, sha_new).expect("anchor should round-trip");
    assert_eq!(back.anchor.commit_hash, sha_new);
    assert_eq!(back.session_refs.len(), 1);
    assert_eq!(back.session_refs[0].session_uid, "suid-1");
    assert_eq!(back.coverage.as_ref().unwrap().tools, vec!["claude"]);

    // Recency listing straight from the index, newest first.
    let listed = list_anchors_by_time(&repo, repo_id);
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0], (2000, sha_new.to_string()));
    assert_eq!(listed[1], (1000, sha_old.to_string()));

    // Unknown repo/sha → clean miss.
    assert!(read_anchor(&repo, repo_id, "ffff").is_none());
    assert!(read_anchor(&repo, "r:other/repo", sha_new).is_none());
}

// ── provenance sessions: merge-on-write ─────────────────────────────────

#[test]
fn provenance_session_merges_on_rewrite() {
    let Some((_tmp, repo)) = init_repo() else {
        return;
    };
    let repo_id = "r:github.com/acme/x";

    let mut first = make_session("suid-merge", 1000);
    first.repos_touched = vec!["repo-a".into()];
    first.turn_count = 2;
    write_provenance_session(&repo, repo_id, &first).unwrap();

    let mut second = make_session("suid-merge", 2000);
    second.repos_touched = vec!["repo-b".into()];
    second.native_session_ids = vec!["native-2".into()];
    second.turn_count = 1; // lower — must NOT regress the stored max
    second.title = Some("rename the parser".into());
    write_provenance_session(&repo, repo_id, &second).unwrap();

    let merged = read_provenance_session(&repo, repo_id, "suid-merge").unwrap();
    assert_eq!(
        merged.repos_touched,
        vec!["repo-a".to_string(), "repo-b".to_string()],
        "repo sets union"
    );
    assert_eq!(
        merged.native_session_ids,
        vec!["native-1".to_string(), "native-2".to_string()],
        "native id sets union"
    );
    assert_eq!(merged.turn_count, 2, "counters take max, never regress");
    assert_eq!(merged.title.as_deref(), Some("rename the parser"));
    assert_eq!(merged.updated_at, 2000);

    // Session recency index sees it.
    let listed = list_sessions_by_time(&repo, repo_id);
    assert_eq!(listed[0].1, "suid-merge");
}

#[test]
fn merge_sessions_is_commutative_and_idempotent() {
    let mut a = make_session("s", 1000);
    a.repos_touched = vec!["x".into()];
    a.turn_count = 5;
    a.ended_at = Some(1500);
    let mut b = make_session("s", 2000);
    b.repos_touched = vec!["y".into()];
    b.turn_count = 3;
    b.model = Some("sonnet".into());

    let ab = merge_sessions(&a, &b);
    let ba = merge_sessions(&b, &a);
    assert_eq!(ab, ba, "merge must be commutative");

    let aa = merge_sessions(&a, &a);
    assert_eq!(aa, a, "merge must be idempotent");

    // LWW: b is newer, so its scalars win.
    assert_eq!(ab.model.as_deref(), Some("sonnet"));
    assert_eq!(ab.turn_count, 5);
    assert_eq!(ab.ended_at, Some(1500));
    assert_eq!(ab.started_at, 900, "earliest start wins");
}

#[test]
fn merge_session_json_handles_serialized_payloads() {
    let a = serde_json::to_string(&make_session("s", 1000)).unwrap();
    let mut newer = make_session("s", 2000);
    newer.repos_touched = vec!["other".into()];
    let b = serde_json::to_string(&newer).unwrap();

    let merged = merge_session_json(&a, &b).expect("valid payloads merge");
    let rec: SessionRecord = serde_json::from_str(&merged).unwrap();
    assert_eq!(rec.updated_at, 2000);
    assert!(rec.repos_touched.contains(&"other".to_string()));
    assert!(rec
        .repos_touched
        .contains(&"r:github.com/acme/x".to_string()));

    assert!(merge_session_json("not json", &b).is_none());
}

// ── provenance turns: immutability ──────────────────────────────────────

#[test]
fn provenance_turn_is_immutable_once_written() {
    let Some((_tmp, repo)) = init_repo() else {
        return;
    };
    let repo_id = "r:github.com/acme/x";

    let turn = TurnRecord {
        schema_version: V2_SCHEMA_VERSION,
        turn_uid: "tuid-1".into(),
        session_uid: "suid-t".into(),
        turn_index: 0,
        native_turn_index: Some(0),
        source: "claude".into(),
        model: Some("opus".into()),
        trigger: Some("fix the bug".into()),
        started_at: Some(100),
        ended_at: Some(200),
        tokens: TurnTokens {
            input: Some(10),
            cache_read: Some(5),
            cache_creation: Some(3),
            output: Some(20),
        },
        tool_names: vec!["Edit".into()],
        capture_gap: false,
    };
    let edits = TurnEdits {
        files: vec![TurnFileSnapshot {
            path: "src/a.rs".into(),
            pre_blob: Some("pre0000".into()),
            post_blob: Some("post000".into()),
            capture_gap: false,
        }],
    };

    assert!(write_provenance_turn(&repo, repo_id, &turn, &edits).unwrap());

    // Re-run with mutated content → no-op, original preserved.
    let mut mutated = turn.clone();
    mutated.trigger = Some("OVERWRITTEN".into());
    assert!(!write_provenance_turn(&repo, repo_id, &mutated, &edits).unwrap());

    let (back, back_edits) = read_provenance_turn(&repo, repo_id, "suid-t", 0).unwrap();
    assert_eq!(back.trigger.as_deref(), Some("fix the bug"));
    assert_eq!(back.tokens.new_work(), 23);
    assert_eq!(back_edits.files.len(), 1);
    assert_eq!(back_edits.files[0].path, "src/a.rs");
}

// ── conversation layer + sanitization choke-point ───────────────────────

#[test]
fn planted_secret_never_reaches_the_orphan_tree() {
    let Some((_tmp, repo)) = init_repo() else {
        return;
    };

    let secret = format!("{}abcdefghij1234567890", "sk_live_");
    let transcript =
        format!(r#"{{"role":"assistant","content":"set api_key = '{secret}' in env"}}"#);
    let tool_calls = format!(r#"[{{"name":"Shell","input":"export TOKEN={secret}"}}]"#);

    assert!(write_conversation_turn(&repo, "suid-sec", 0, &transcript, &tool_calls).unwrap());

    // Read EVERY blob on the branch and assert the secret is nowhere.
    let tree = git_in(&repo, &["ls-tree", "-r", "--name-only", BRANCH]).unwrap();
    for path in tree.lines() {
        let content = read_from_branch_named(&repo, BRANCH, path).unwrap_or_default();
        assert!(
            !content.contains(&secret),
            "secret leaked into {path}: {content}"
        );
    }

    // The sanitized payload is still retrievable and structurally intact.
    let (transcript_back, tool_calls_back) = read_conversation_turn(&repo, "suid-sec", 0).unwrap();
    assert!(transcript_back.contains("[REDACTED]"));
    assert!(tool_calls_back.contains("[REDACTED]"));
}

#[test]
fn conversation_turn_immutable_and_session_indexed() {
    let Some((_tmp, repo)) = init_repo() else {
        return;
    };

    let mut session = make_session("suid-conv", 1000);
    session.repos_touched = vec!["repo-x".into(), "repo-y".into()];
    write_conversation_session(&repo, &session).unwrap();

    assert!(write_conversation_turn(&repo, "suid-conv", 0, "{\"t\":1}", "[]").unwrap());
    assert!(
        !write_conversation_turn(&repo, "suid-conv", 0, "{\"t\":\"clobber\"}", "[]").unwrap(),
        "conversation turns are immutable"
    );

    let (transcript, _) = read_conversation_turn(&repo, "suid-conv", 0).unwrap();
    assert_eq!(transcript.trim(), "{\"t\":1}");

    // Global session index: uid → home + repos.
    let index = read_sessions_index(&repo);
    let entry = index.get("suid-conv").expect("session indexed");
    let repos: Vec<&str> = entry["repos"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(repos.contains(&"repo-x") && repos.contains(&"repo-y"));

    let back = read_conversation_session(&repo, "suid-conv").unwrap();
    assert_eq!(back.session_uid, "suid-conv");
}

#[test]
fn publisher_block_on_secret_mode_refuses() {
    let mut publisher = Publisher::new("/tmp/x");
    publisher.block_on_secret = true;

    let secret_text = format!("api_key = '{}abcdefghij1234567890'", "sk_live_");
    assert!(
        publisher.publish(&secret_text).is_err(),
        "block-on-secret must refuse instead of redacting"
    );
    assert_eq!(publisher.publish("clean text").unwrap(), "clean text");
}

// ── continuation chains ("one long session") ───────────────────────────

#[test]
fn chain_root_follows_resume_and_compaction_links() {
    let Some((_tmp, repo)) = init_repo() else {
        return;
    };

    // root ← (resumed) mid ← (compacted) leaf
    let root = make_session("suid-root", 100);
    write_conversation_session(&repo, &root).unwrap();

    let mut mid = make_session("suid-mid", 200);
    mid.lineage.resumed_from = Some("suid-root".into());
    write_conversation_session(&repo, &mid).unwrap();

    let mut leaf = make_session("suid-leaf", 300);
    leaf.lineage.compacted_from = Some("suid-mid".into());

    assert_eq!(chain_root_uid(&repo, &leaf), "suid-root");
    assert_eq!(chain_root_uid(&repo, &mid), "suid-root");
    assert_eq!(chain_root_uid(&repo, &root), "suid-root");
}

#[test]
fn chain_root_guards_cycles_and_trusts_external_links() {
    let Some((_tmp, repo)) = init_repo() else {
        return;
    };

    // Cycle: a → b → a must terminate.
    let mut a = make_session("suid-a", 100);
    a.lineage.resumed_from = Some("suid-b".into());
    write_conversation_session(&repo, &a).unwrap();
    let mut b = make_session("suid-b", 200);
    b.lineage.resumed_from = Some("suid-a".into());
    write_conversation_session(&repo, &b).unwrap();
    let root = chain_root_uid(&repo, &a);
    assert!(root == "suid-a" || root == "suid-b", "cycle must terminate");

    // Link to a session not in this store → trust the link target.
    let mut orphan_link = make_session("suid-x", 300);
    orphan_link.lineage.resumed_from = Some("suid-elsewhere".into());
    assert_eq!(chain_root_uid(&repo, &orphan_link), "suid-elsewhere");

    // Subagents are NOT chained.
    let mut sub = make_session("suid-sub", 400);
    sub.lineage.parent_session_uid = Some("suid-a".into());
    assert_eq!(chain_root_uid(&repo, &sub), "suid-sub");
}

// ── maintenance ─────────────────────────────────────────────────────────

#[test]
fn squash_to_tip_collapses_history_and_preserves_content() {
    let Some((_tmp, repo)) = init_repo() else {
        return;
    };
    let repo_id = "r:github.com/acme/x";

    for i in 0..3 {
        let sha = format!("{i}{i}1100000000000000000000000000000000000{i}");
        write_anchor(
            &repo,
            repo_id,
            &AnchorRecord {
                anchor: make_anchor(&sha, 1000 + i),
                session_refs: Vec::new(),
                session_links: Vec::new(),
                coverage: None,
            },
            None,
        )
        .unwrap();
    }
    assert!(branch_depth(&repo) > 1, "history accumulated");

    squash_to_tip(&repo).unwrap();
    assert_eq!(branch_depth(&repo), 1, "history squashed to a single root");

    // Content fully preserved.
    for i in 0..3 {
        let sha = format!("{i}{i}1100000000000000000000000000000000000{i}");
        assert!(
            read_anchor(&repo, repo_id, &sha).is_some(),
            "anchor {i} must survive the squash"
        );
    }
    assert_eq!(list_anchors_by_time(&repo, repo_id).len(), 3);

    // Idempotent.
    squash_to_tip(&repo).unwrap();
    assert_eq!(branch_depth(&repo), 1);
}

// ── concurrent-writer convergence through the sync replay ──────────────

#[test]
fn diverged_session_writers_converge_via_replay_merge() {
    let Some((_tmp, repo)) = init_repo() else {
        return;
    };
    let repo_id = "r:github.com/acme/x";

    // Common base.
    write_provenance_session(&repo, repo_id, &make_session("suid-div", 500)).unwrap();
    let base = git_in(&repo, &["rev-parse", BRANCH]).unwrap();

    // Writer A: adds repo-a, writes a turn.
    let mut a = make_session("suid-div", 1000);
    a.repos_touched = vec!["repo-a".into()];
    write_provenance_session(&repo, repo_id, &a).unwrap();
    let turn_a = TurnRecord {
        turn_uid: "t-a".into(),
        session_uid: "suid-div".into(),
        turn_index: 0,
        ..Default::default()
    };
    write_provenance_turn(&repo, repo_id, &turn_a, &TurnEdits::default()).unwrap();
    let tip_a = git_in(&repo, &["rev-parse", BRANCH]).unwrap();

    // Writer B: diverges from base, adds repo-b, writes a different turn.
    git_in(
        &repo,
        &["update-ref", &format!("refs/heads/{BRANCH}"), &base],
    )
    .unwrap();
    let mut b = make_session("suid-div", 2000);
    b.repos_touched = vec!["repo-b".into()];
    write_provenance_session(&repo, repo_id, &b).unwrap();
    let turn_b = TurnRecord {
        turn_uid: "t-b".into(),
        session_uid: "suid-div".into(),
        turn_index: 1,
        ..Default::default()
    };
    write_provenance_turn(&repo, repo_id, &turn_b, &TurnEdits::default()).unwrap();
    let tip_b = git_in(&repo, &["rev-parse", BRANCH]).unwrap();

    // Reconcile A's tip ("local") with B's tip ("remote").
    git_in(
        &repo,
        &["update-ref", &format!("refs/heads/{BRANCH}"), &tip_a],
    )
    .unwrap();
    crate::git::orphan::sync::replay_local_files(&repo, &tip_a, &tip_b, BRANCH).unwrap();

    // Both writers' turns survive (immutable files union)...
    assert!(read_provenance_turn(&repo, repo_id, "suid-div", 0).is_some());
    assert!(read_provenance_turn(&repo, repo_id, "suid-div", 1).is_some());

    // ...and the mutable session record merged per-field instead of one
    // side silently winning.
    let merged = read_provenance_session(&repo, repo_id, "suid-div").unwrap();
    assert!(
        merged.repos_touched.contains(&"repo-a".to_string())
            && merged.repos_touched.contains(&"repo-b".to_string()),
        "both writers' repo sets must survive reconcile: {:?}",
        merged.repos_touched
    );
    assert_eq!(merged.updated_at, 2000);
}
