//! `oobo doctor` — capture-health diagnostics.
//!
//! The question doctor answers is not "is oobo configured" but "is the
//! evidence pipeline actually flowing": hooks installed AND firing per
//! tool (last-event-seen), git hooks present in this repo, spool drained,
//! stores reachable. session-start self-heals missing git hooks (see
//! `hooks::self_heal_project_hooks`); doctor reports what it sees.

use crate::cli::OutputMode;
use crate::config::Config;
use crate::error::CmdResult;

#[derive(serde::Serialize)]
struct ToolHealth {
    tool: String,
    /// Hook config file contains the oobo command.
    installed: bool,
    /// Last hook event received from this tool (proof of firing).
    #[serde(skip_serializing_if = "Option::is_none")]
    last_event: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_seen_at: Option<i64>,
}

#[derive(serde::Serialize)]
struct RepoHealth {
    root: String,
    enabled: bool,
    git_hooks_installed: bool,
    spool_pending: bool,
    v1_branch: bool,
    v2_branch: bool,
    live_sessions: usize,
}

pub fn run(cfg: &Config, mode: OutputMode) -> CmdResult {
    let tools = tool_health();
    let repo = crate::git::proxy::project_root(cfg).map(|root| repo_health(&root));
    let no_repo_edits = no_repo_ledger_entries();

    if mode == OutputMode::Json {
        crate::utils::print_json(&serde_json::json!({
            "tools": tools,
            "repo": repo,
            "no_repo_edits": no_repo_edits,
        }));
    } else {
        {
            println!("tools (hooks installed / firing):");
            if tools.is_empty() {
                println!("  no supported tool configs found.");
            }
            for t in &tools {
                let installed = if t.installed {
                    "installed"
                } else {
                    "NOT INSTALLED"
                };
                let firing = match (t.last_event.as_deref(), t.last_seen_at) {
                    (Some(ev), Some(ts)) => format!("last event {ev} at {}", fmt_ts(ts)),
                    _ => "never seen an event".to_string(),
                };
                println!("  {:<10} {installed:<14} {firing}", t.tool);
            }
            if let Some(r) = &repo {
                println!("\nrepo {}:", r.root);
                println!("  anchors enabled:     {}", yn(r.enabled));
                println!("  git hooks installed: {}", yn(r.git_hooks_installed));
                println!(
                    "  spool:               {}",
                    if r.spool_pending {
                        "pending entries (worker will drain)"
                    } else {
                        "drained"
                    }
                );
                println!("  anchors/v1 branch:   {}", yn(r.v1_branch));
                println!("  anchors/v2 branch:   {}", yn(r.v2_branch));
                println!("  live sessions:       {}", r.live_sessions);
                if r.enabled && !r.git_hooks_installed {
                    println!(
                        "\n  hint: git hooks missing — the next session-start self-heals them,\n  \
                         or run `oobo on` to reinstall now."
                    );
                }
            } else {
                println!("\nnot inside a git repository — repo checks skipped.");
            }
            if no_repo_edits > 0 {
                println!(
                    "\n{no_repo_edits} edit(s) captured outside any git repo \
                     (~/.oobo/state/no-repo-ledger.jsonl)."
                );
            }
        }
    }
    Ok(0)
}

/// Count of file mutations captured outside any git repository — work
/// that exists but can never be claimed by a commit.
fn no_repo_ledger_entries() -> usize {
    let path = crate::paths::oobo_home()
        .join("state")
        .join("no-repo-ledger.jsonl");
    std::fs::read_to_string(path).map_or(0, |content| {
        content.lines().filter(|l| !l.trim().is_empty()).count()
    })
}

/// Per-tool: hook config present + last-event-seen liveness marker.
fn tool_health() -> Vec<ToolHealth> {
    let liveness: std::collections::HashMap<String, crate::hooks::ToolLiveness> =
        crate::hooks::read_tool_liveness()
            .into_iter()
            .map(|l| (l.tool.clone(), l))
            .collect();

    let configs: &[(&str, &str)] = &[
        ("cursor", ".cursor/hooks.json"),
        ("claude", ".claude/settings.json"),
        ("gemini", ".gemini/settings.json"),
        ("continue", ".continue/settings.json"),
        ("droid", ".factory/settings.json"),
    ];

    let home = dirs::home_dir().unwrap_or_default();
    let mut out: Vec<ToolHealth> = Vec::new();
    let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (tool, rel) in configs {
        let path = home.join(rel);
        let exists = path.exists();
        let installed = exists
            && std::fs::read_to_string(&path)
                .map(|c| c.contains("oobo hooks agent"))
                .unwrap_or(false);
        // Tools with no config file at all are simply not present on
        // this machine — only report them if we've seen events (which
        // would mean hooks fire through another mechanism).
        let norm = crate::core::tool::normalize_source(tool).to_string();
        let live = liveness.get(&norm).or_else(|| liveness.get(*tool));
        if !exists && live.is_none() {
            continue;
        }
        covered.insert(norm.clone());
        covered.insert((*tool).to_string());
        out.push(ToolHealth {
            tool: (*tool).to_string(),
            installed,
            last_event: live.map(|l| l.event.clone()),
            last_seen_at: live.map(|l| l.seen_at),
        });
    }

    // Tools that fired events but have no known config file (ACP tools,
    // custom integrations): firing is the stronger signal — report them.
    for (tool, l) in &liveness {
        if !covered.contains(tool) {
            out.push(ToolHealth {
                tool: tool.clone(),
                installed: true,
                last_event: Some(l.event.clone()),
                last_seen_at: Some(l.seen_at),
            });
        }
    }

    out.sort_by(|a, b| a.tool.cmp(&b.tool));
    out
}

fn repo_health(root: &str) -> RepoHealth {
    let hook = crate::git::detect::resolve_git_common_dir(root)
        .join("hooks")
        .join("post-commit");
    let git_hooks_installed = std::fs::read_to_string(&hook)
        .map(|c| c.contains("oobo"))
        .unwrap_or(false);

    RepoHealth {
        root: root.to_string(),
        enabled: crate::project_config::is_enabled(root),
        git_hooks_installed,
        spool_pending: crate::git::spool::has_pending(root),
        v1_branch: crate::git::orphan::branch_exists(root),
        v2_branch: crate::git::orphan::v2::branch_exists(root),
        live_sessions: crate::hooks::store::list_for_project(root).len(),
    }
}

fn yn(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "NO"
    }
}

fn fmt_ts(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0).map_or_else(
        || ts.to_string(),
        |dt| dt.format("%Y-%m-%d %H:%M UTC").to_string(),
    )
}
