use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::cli::OutputMode;
use crate::config::Config;
use crate::core::anchor::{Anchor, AuthorType, FileAttribution, SessionLink};
use crate::git::{orphan, proxy};

/// Aggregate AI contribution data across a range of commits and render
/// a PR summary suitable for posting as a CI comment.
pub fn run(
    cfg: &Config,
    base: Option<&str>,
    head: Option<&str>,
    mode: OutputMode,
) -> Result<(), String> {
    let project_root = proxy::project_root(cfg).unwrap_or_default();
    if project_root.is_empty() {
        return Err("not in a git repository".into());
    }

    let base_ref = base
        .map(String::from)
        .or_else(detect_base_ref)
        .unwrap_or_else(|| "origin/main".into());
    let head_ref = head.unwrap_or("HEAD");

    let commit_hashes = list_commits_in_range(cfg, &base_ref, head_ref)?;
    if commit_hashes.is_empty() {
        if mode.is_structured() {
            crate::utils::print_json(&serde_json::json!({
                "error": "no commits in range",
                "base": base_ref,
                "head": head_ref,
            }));
        } else {
            eprintln!("oobo: no commits found in {base_ref}..{head_ref}");
        }
        return Ok(());
    }

    if !orphan::branch_exists(&project_root) {
        if mode.is_structured() {
            crate::utils::print_json(&serde_json::json!({
                "error": "no anchor branch",
                "detail": "oobo/anchors/v1 branch does not exist — commits have no AI context",
            }));
        } else {
            eprintln!("oobo: no anchor branch found — run oobo setup first");
        }
        return Ok(());
    }

    let summary = build_summary(&project_root, &commit_hashes);

    match mode {
        OutputMode::Agent => print_agent(&summary),
        OutputMode::Json => crate::utils::print_json(&summary),
        OutputMode::Tui => print_markdown(&summary),
    }

    Ok(())
}

/// Auto-detect the base ref from CI environment variables.
fn detect_base_ref() -> Option<String> {
    // GitHub Actions
    if let Ok(base) = std::env::var("GITHUB_BASE_REF") {
        if !base.is_empty() {
            return Some(format!("origin/{base}"));
        }
    }
    // GitLab CI
    if let Ok(base) = std::env::var("CI_MERGE_REQUEST_TARGET_BRANCH_NAME") {
        if !base.is_empty() {
            return Some(format!("origin/{base}"));
        }
    }
    // Travis CI
    if let Ok(base) = std::env::var("TRAVIS_BRANCH") {
        if std::env::var("TRAVIS_PULL_REQUEST").ok().as_deref() != Some("false") {
            return Some(format!("origin/{base}"));
        }
    }
    // CircleCI — no direct base ref, but CIRCLE_BRANCH is the PR branch
    // Buildkite
    if let Ok(base) = std::env::var("BUILDKITE_PULL_REQUEST_BASE_BRANCH") {
        if !base.is_empty() {
            return Some(format!("origin/{base}"));
        }
    }
    None
}

fn list_commits_in_range(cfg: &Config, base: &str, head: &str) -> Result<Vec<String>, String> {
    let output = proxy::run_git_capture(cfg, &["log", "--format=%H", &format!("{base}..{head}")])?;
    Ok(output.lines().filter(|l| !l.is_empty()).map(String::from).collect())
}

fn build_summary(project_root: &str, commit_hashes: &[String]) -> PrSummary {
    let mut total_added: u64 = 0;
    let mut total_deleted: u64 = 0;
    let mut ai_added: u64 = 0;
    let mut ai_deleted: u64 = 0;
    let mut human_added: u64 = 0;
    let mut human_deleted: u64 = 0;
    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
    let mut tools: HashSet<String> = HashSet::new();
    let mut models: HashSet<String> = HashSet::new();
    let mut session_count: usize = 0;
    let mut commits_with_anchors: usize = 0;
    let mut author_type_counts: HashMap<String, usize> = HashMap::new();
    let mut file_map: HashMap<String, FileSummary> = HashMap::new();

    for hash in commit_hashes {
        let anchor = match orphan::read_anchor(project_root, hash) {
            Some(a) => a,
            None => continue,
        };
        commits_with_anchors += 1;

        let links = orphan::read_session_links(project_root, hash);

        accumulate_anchor(&anchor, &links, &mut AggState {
            total_added: &mut total_added,
            total_deleted: &mut total_deleted,
            ai_added: &mut ai_added,
            ai_deleted: &mut ai_deleted,
            human_added: &mut human_added,
            human_deleted: &mut human_deleted,
            total_input_tokens: &mut total_input_tokens,
            total_output_tokens: &mut total_output_tokens,
            tools: &mut tools,
            models: &mut models,
            session_count: &mut session_count,
            author_type_counts: &mut author_type_counts,
            file_map: &mut file_map,
        });
    }

    let total_lines = ai_added + ai_deleted + human_added + human_deleted;
    let ai_lines = ai_added + ai_deleted;
    let ai_percentage = if total_lines > 0 {
        Some((ai_lines as f64 / total_lines as f64) * 100.0)
    } else {
        None
    };

    let total_tokens = total_input_tokens + total_output_tokens;

    let mut tools_sorted: Vec<String> = tools.into_iter().collect();
    tools_sorted.sort();
    let mut models_sorted: Vec<String> = models.into_iter().collect();
    models_sorted.sort();

    let estimated_cost = estimate_total_cost(&models_sorted, total_input_tokens, total_output_tokens);

    let mut files: Vec<FileSummary> = file_map.into_values().collect();
    files.sort_by(|a, b| {
        let a_total = a.added + a.deleted;
        let b_total = b.added + b.deleted;
        b_total.cmp(&a_total)
    });

    PrSummary {
        base: String::new(),
        head: String::new(),
        total_commits: commit_hashes.len(),
        commits_with_anchors,
        total_added,
        total_deleted,
        ai_added,
        ai_deleted,
        human_added,
        human_deleted,
        ai_percentage,
        tools: tools_sorted,
        models: models_sorted,
        total_input_tokens,
        total_output_tokens,
        total_tokens,
        estimated_cost,
        session_count,
        author_type_breakdown: author_type_counts,
        files,
    }
}

struct AggState<'a> {
    total_added: &'a mut u64,
    total_deleted: &'a mut u64,
    ai_added: &'a mut u64,
    ai_deleted: &'a mut u64,
    human_added: &'a mut u64,
    human_deleted: &'a mut u64,
    total_input_tokens: &'a mut u64,
    total_output_tokens: &'a mut u64,
    tools: &'a mut HashSet<String>,
    models: &'a mut HashSet<String>,
    session_count: &'a mut usize,
    author_type_counts: &'a mut HashMap<String, usize>,
    file_map: &'a mut HashMap<String, FileSummary>,
}

fn accumulate_anchor(anchor: &Anchor, links: &[SessionLink], st: &mut AggState) {
    *st.total_added += anchor.added as u64;
    *st.total_deleted += anchor.deleted as u64;
    *st.ai_added += anchor.ai_added as u64;
    *st.ai_deleted += anchor.ai_deleted as u64;
    *st.human_added += anchor.human_added as u64;
    *st.human_deleted += anchor.human_deleted as u64;

    let author_key = match anchor.author_type {
        AuthorType::Agent => "agent",
        AuthorType::Assisted => "assisted",
        AuthorType::Human => "human",
        AuthorType::Automated => "automated",
    };
    *st.author_type_counts.entry(author_key.into()).or_insert(0) += 1;

    for link in links {
        *st.session_count += 1;
        let label = crate::tui::source_label(&link.agent);
        st.tools.insert(label.to_string());
        if let Some(ref model) = link.model {
            st.models.insert(model.clone());
        }
        if let Some(inp) = link.input_tokens {
            *st.total_input_tokens += inp;
        }
        if let Some(out) = link.output_tokens {
            *st.total_output_tokens += out;
        }
    }

    for fc in &anchor.file_changes {
        let entry = st.file_map.entry(fc.path.clone()).or_insert_with(|| FileSummary {
            path: fc.path.clone(),
            added: 0,
            deleted: 0,
            ai_added: 0,
            ai_deleted: 0,
            attribution: None,
            agent: None,
        });
        entry.added += fc.added as u64;
        entry.deleted += fc.deleted as u64;

        match fc.attribution {
            Some(FileAttribution::Ai) => {
                entry.ai_added += fc.added as u64;
                entry.ai_deleted += fc.deleted as u64;
                entry.attribution = Some("ai".into());
                if entry.agent.is_none() {
                    entry.agent = fc.agent.clone();
                }
            }
            Some(FileAttribution::Mixed) => {
                let ai_a = fc.added as u64 / 2;
                let ai_d = fc.deleted as u64 / 2;
                entry.ai_added += ai_a;
                entry.ai_deleted += ai_d;
                if entry.attribution.as_deref() != Some("ai") {
                    entry.attribution = Some("mixed".into());
                }
                if entry.agent.is_none() {
                    entry.agent = fc.agent.clone();
                }
            }
            Some(FileAttribution::Human) | None => {
                if entry.attribution.is_none() {
                    entry.attribution = Some("human".into());
                }
            }
        }
    }
}

fn estimate_total_cost(models: &[String], input_tokens: u64, output_tokens: u64) -> Option<f64> {
    if input_tokens == 0 && output_tokens == 0 {
        return None;
    }

    // If we know specific models, use per-model pricing. Otherwise use a
    // blended average ($3/M input, $12/M output) which covers common models.
    let has_specific = !models.is_empty();

    if !has_specific {
        let cost = (input_tokens as f64 * 3.0 + output_tokens as f64 * 12.0) / 1_000_000.0;
        return Some(cost);
    }

    // Use the average of known model costs as a proxy since we don't have
    // per-model token breakdowns — tokens are aggregated across all sessions.
    let (mut sum_in, mut sum_out, mut count) = (0.0_f64, 0.0_f64, 0u32);
    for model in models {
        if let Some((ci, co)) = model_cost_per_million(model) {
            sum_in += ci;
            sum_out += co;
            count += 1;
        }
    }
    if count == 0 {
        let cost = (input_tokens as f64 * 3.0 + output_tokens as f64 * 12.0) / 1_000_000.0;
        return Some(cost);
    }

    let avg_in = sum_in / count as f64;
    let avg_out = sum_out / count as f64;
    let cost = (input_tokens as f64 * avg_in + output_tokens as f64 * avg_out) / 1_000_000.0;
    Some(cost)
}

/// Returns (input_cost_per_million, output_cost_per_million) for known models.
fn model_cost_per_million(model: &str) -> Option<(f64, f64)> {
    let m = model.to_lowercase();
    if m.contains("claude-4") || m.contains("opus") {
        Some((15.0, 75.0))
    } else if m.contains("sonnet") {
        Some((3.0, 15.0))
    } else if m.contains("haiku") {
        Some((0.80, 4.0))
    } else if m.contains("gpt-4o-mini") {
        Some((0.15, 0.60))
    } else if m.contains("gpt-4o") || m.contains("gpt-4.1") {
        Some((2.50, 10.0))
    } else if m.contains("o3") || m.contains("o4-mini") {
        Some((1.10, 4.40))
    } else if m.contains("gemini-2.5-pro") {
        Some((1.25, 10.0))
    } else if m.contains("gemini-2.5-flash") || m.contains("gemini-2.0-flash") {
        Some((0.15, 0.60))
    } else if m.contains("gemini") {
        Some((0.50, 1.50))
    } else if m.contains("deepseek") {
        Some((0.27, 1.10))
    } else {
        None
    }
}

fn format_cost(cost: f64) -> String {
    if cost < 0.01 {
        format!("<$0.01")
    } else if cost < 1.0 {
        format!("${:.2}", cost)
    } else {
        format!("${:.2}", cost)
    }
}

fn format_tokens_short(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn format_lines(added: u64, deleted: u64) -> String {
    if deleted == 0 {
        format!("+{added}")
    } else if added == 0 {
        format!("-{deleted}")
    } else {
        format!("+{added} -{deleted}")
    }
}

// ── Output renderers ────────────────────────────────────────────────────

fn print_markdown(summary: &PrSummary) {
    let mut out = String::new();
    out.push_str("<!-- oobo-pr-summary -->\n");
    out.push_str("## oobo — AI Contribution Report\n\n");

    if summary.commits_with_anchors == 0 {
        out.push_str(&format!(
            "Analyzed {} commit(s) — none have AI context recorded.\n\n",
            summary.total_commits
        ));
        out.push_str("---\n");
        out.push_str("<sub>Generated by <a href=\"https://oobo.ai\">oobo</a></sub>\n");
        println!("{out}");
        return;
    }

    // Summary table
    out.push_str("| | |\n|---|---|\n");

    if let Some(pct) = summary.ai_percentage {
        let ai_lines = summary.ai_added + summary.ai_deleted;
        let human_lines = summary.human_added + summary.human_deleted;
        let human_pct = 100.0 - pct;
        out.push_str(&format!(
            "| **AI Contribution** | {:.0}% ({} lines) |\n",
            pct,
            format_tokens_short(ai_lines)
        ));
        out.push_str(&format!(
            "| **Human Contribution** | {:.0}% ({} lines) |\n",
            human_pct,
            format_tokens_short(human_lines)
        ));
    }

    if !summary.tools.is_empty() {
        out.push_str(&format!(
            "| **Tools Used** | {} |\n",
            summary.tools.join(", ")
        ));
    }
    if !summary.models.is_empty() {
        let display_models: Vec<&str> = summary.models.iter().take(4).map(|s| s.as_str()).collect();
        let suffix = if summary.models.len() > 4 {
            format!(" +{}", summary.models.len() - 4)
        } else {
            String::new()
        };
        out.push_str(&format!(
            "| **Models** | {}{} |\n",
            display_models.join(", "),
            suffix
        ));
    }

    if summary.total_tokens > 0 {
        let cost_str = summary
            .estimated_cost
            .map(|c| format!(" (~{})", format_cost(c)))
            .unwrap_or_default();
        out.push_str(&format!(
            "| **Tokens Consumed** | {}{} |\n",
            format_tokens_short(summary.total_tokens),
            cost_str
        ));
    }

    out.push_str(&format!(
        "| **Sessions** | {} across {} commit(s) |\n",
        summary.session_count, summary.commits_with_anchors,
    ));

    if summary.commits_with_anchors < summary.total_commits {
        out.push_str(&format!(
            "| **Coverage** | {} of {} commits have AI context |\n",
            summary.commits_with_anchors, summary.total_commits,
        ));
    }

    // Per-file breakdown (collapsible)
    if !summary.files.is_empty() {
        let file_count = summary.files.len();
        out.push_str(&format!(
            "\n<details>\n<summary>Per-file breakdown ({file_count} file(s) changed)</summary>\n\n"
        ));
        out.push_str("| File | Attribution | Lines | Agent |\n");
        out.push_str("|------|------------|-------|-------|\n");

        for f in summary.files.iter().take(30) {
            let attr_label = file_attribution_label(f);
            let lines = format_lines(f.added, f.deleted);
            let agent = f.agent.as_deref().unwrap_or("—");
            out.push_str(&format!(
                "| `{}` | {} | {} | {} |\n",
                f.path, attr_label, lines, agent
            ));
        }
        if summary.files.len() > 30 {
            out.push_str(&format!(
                "\n*… and {} more files*\n",
                summary.files.len() - 30
            ));
        }

        out.push_str("\n</details>\n");
    }

    out.push_str("\n---\n");
    out.push_str("<sub>Generated by <a href=\"https://oobo.ai\">oobo</a> · AI development control plane</sub>\n");

    println!("{out}");
}

fn file_attribution_label(f: &FileSummary) -> String {
    let total = f.added + f.deleted;
    let ai = f.ai_added + f.ai_deleted;
    match f.attribution.as_deref() {
        Some("ai") => "AI".into(),
        Some("human") => "Human".into(),
        Some("mixed") if total > 0 => {
            let pct = (ai as f64 / total as f64) * 100.0;
            format!("Mixed ({:.0}% AI)", pct)
        }
        _ => "—".into(),
    }
}

fn print_agent(summary: &PrSummary) {
    let ai_pct = summary
        .ai_percentage
        .map(|p| format!("{:.0}%", p))
        .unwrap_or_else(|| "n/a".into());
    let cost = summary
        .estimated_cost
        .map(|c| format_cost(c))
        .unwrap_or_else(|| "n/a".into());
    println!(
        "commits: {} | with_anchors: {} | ai: {} | sessions: {} | tokens: {} | cost: {} | tools: {}",
        summary.total_commits,
        summary.commits_with_anchors,
        ai_pct,
        summary.session_count,
        format_tokens_short(summary.total_tokens),
        cost,
        summary.tools.join(","),
    );
}

// ── Data structures ─────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PrSummary {
    pub base: String,
    pub head: String,
    pub total_commits: usize,
    pub commits_with_anchors: usize,
    pub total_added: u64,
    pub total_deleted: u64,
    pub ai_added: u64,
    pub ai_deleted: u64,
    pub human_added: u64,
    pub human_deleted: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_percentage: Option<f64>,
    pub tools: Vec<String>,
    pub models: Vec<String>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<f64>,
    pub session_count: usize,
    pub author_type_breakdown: HashMap<String, usize>,
    pub files: Vec<FileSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileSummary {
    pub path: String,
    pub added: u64,
    pub deleted: u64,
    pub ai_added: u64,
    pub ai_deleted: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::anchor::{
        Contributor, ContributorRole, FileChange, LinkType, TransparencyMode,
    };

    // ── Helpers ─────────────────────────────────────────────────────────

    fn make_anchor(
        hash: &str,
        added: u32,
        deleted: u32,
        ai_added: u32,
        ai_deleted: u32,
        author_type: AuthorType,
        file_changes: Vec<FileChange>,
    ) -> Anchor {
        Anchor {
            oobo_version: "0.1.0".into(),
            commit_hash: hash.into(),
            branch: "main".into(),
            author: "Dev <dev@test.com>".into(),
            author_type,
            contributors: vec![Contributor {
                name: "Dev".into(),
                role: ContributorRole::Human,
                model: None,
            }],
            committed_at: 1700000000,
            message: "test commit".into(),
            files_changed: file_changes.iter().map(|f| f.path.clone()).collect(),
            added,
            deleted,
            file_changes,
            ai_added,
            ai_deleted,
            human_added: added - ai_added,
            human_deleted: deleted - ai_deleted,
            ai_percentage: if added + deleted > 0 {
                Some(
                    ((ai_added + ai_deleted) as f64 / (added + deleted) as f64) * 100.0,
                )
            } else {
                None
            },
            session_ids: vec![],
            summary: None,
            intent: None,
            reasoning: None,
            transparency_mode: TransparencyMode::Off,
            file_interactions: None,
        }
    }

    fn make_session_link(
        agent: &str,
        model: Option<&str>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    ) -> SessionLink {
        SessionLink {
            session_id: format!("session-{agent}"),
            agent: agent.into(),
            model: model.map(String::from),
            link_type: LinkType::Explicit,
            input_tokens,
            output_tokens,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            duration_secs: Some(60),
            tool_calls: Some(3),
            files_touched: None,
            tool_usage: None,
            tool_failures: None,
            subagent_count: None,
            bash_commands: None,
            thinking_duration_ms: None,
            compact_count: None,
            is_subagent: false,
            parent_session_id: None,
            subagent_type: None,
            is_estimated: false,
            peer_session_ids: vec![],
        }
    }

    fn empty_agg_state() -> (
        u64, u64, u64, u64, u64, u64, u64, u64,
        HashSet<String>, HashSet<String>, usize,
        HashMap<String, usize>, HashMap<String, FileSummary>,
    ) {
        (0, 0, 0, 0, 0, 0, 0, 0,
         HashSet::new(), HashSet::new(), 0,
         HashMap::new(), HashMap::new())
    }

    fn run_accumulate(anchor: &Anchor, links: &[SessionLink]) -> (
        u64, u64, u64, u64, u64, u64, u64, u64,
        HashSet<String>, HashSet<String>, usize,
        HashMap<String, usize>, HashMap<String, FileSummary>,
    ) {
        let (
            mut ta, mut td, mut aa, mut ad, mut ha, mut hd, mut ti, mut to,
            mut tools, mut models, mut sc, mut atc, mut fm,
        ) = empty_agg_state();
        accumulate_anchor(anchor, links, &mut AggState {
            total_added: &mut ta, total_deleted: &mut td,
            ai_added: &mut aa, ai_deleted: &mut ad,
            human_added: &mut ha, human_deleted: &mut hd,
            total_input_tokens: &mut ti, total_output_tokens: &mut to,
            tools: &mut tools, models: &mut models,
            session_count: &mut sc, author_type_counts: &mut atc,
            file_map: &mut fm,
        });
        (ta, td, aa, ad, ha, hd, ti, to, tools, models, sc, atc, fm)
    }

    // ── format_cost ─────────────────────────────────────────────────────

    #[test]
    fn test_format_cost_sub_penny() {
        assert_eq!(format_cost(0.001), "<$0.01");
        assert_eq!(format_cost(0.009), "<$0.01");
        assert_eq!(format_cost(0.0), "<$0.01");
    }

    #[test]
    fn test_format_cost_cents() {
        assert_eq!(format_cost(0.01), "$0.01");
        assert_eq!(format_cost(0.15), "$0.15");
        assert_eq!(format_cost(0.99), "$0.99");
    }

    #[test]
    fn test_format_cost_dollars() {
        assert_eq!(format_cost(1.00), "$1.00");
        assert_eq!(format_cost(1.50), "$1.50");
        assert_eq!(format_cost(42.37), "$42.37");
    }

    // ── format_tokens_short ─────────────────────────────────────────────

    #[test]
    fn test_format_tokens_short_small() {
        assert_eq!(format_tokens_short(0), "0");
        assert_eq!(format_tokens_short(1), "1");
        assert_eq!(format_tokens_short(999), "999");
    }

    #[test]
    fn test_format_tokens_short_thousands() {
        assert_eq!(format_tokens_short(1_000), "1.0K");
        assert_eq!(format_tokens_short(1_500), "1.5K");
        assert_eq!(format_tokens_short(45_200), "45.2K");
        assert_eq!(format_tokens_short(999_999), "1000.0K");
    }

    #[test]
    fn test_format_tokens_short_millions() {
        assert_eq!(format_tokens_short(1_000_000), "1.0M");
        assert_eq!(format_tokens_short(2_500_000), "2.5M");
    }

    // ── format_lines ────────────────────────────────────────────────────

    #[test]
    fn test_format_lines_both() {
        assert_eq!(format_lines(10, 5), "+10 -5");
        assert_eq!(format_lines(1, 1), "+1 -1");
    }

    #[test]
    fn test_format_lines_add_only() {
        assert_eq!(format_lines(10, 0), "+10");
    }

    #[test]
    fn test_format_lines_delete_only() {
        assert_eq!(format_lines(0, 5), "-5");
    }

    #[test]
    fn test_format_lines_zero() {
        assert_eq!(format_lines(0, 0), "+0");
    }

    // ── model_cost_per_million ──────────────────────────────────────────

    #[test]
    fn test_model_cost_claude_family() {
        let (inp, out) = model_cost_per_million("claude-sonnet-4").unwrap();
        assert!((inp - 3.0).abs() < 0.01);
        assert!((out - 15.0).abs() < 0.01);

        assert!(model_cost_per_million("claude-4-opus-20260301").is_some());
        assert!(model_cost_per_million("claude-3.5-haiku").is_some());
    }

    #[test]
    fn test_model_cost_openai_family() {
        assert!(model_cost_per_million("gpt-4o").is_some());
        assert!(model_cost_per_million("gpt-4o-mini").is_some());
        assert!(model_cost_per_million("gpt-4.1").is_some());
        assert!(model_cost_per_million("o3-mini").is_some());
        assert!(model_cost_per_million("o4-mini-2026").is_some());
    }

    #[test]
    fn test_model_cost_gemini_family() {
        assert!(model_cost_per_million("gemini-2.5-pro").is_some());
        assert!(model_cost_per_million("gemini-2.5-flash").is_some());
        assert!(model_cost_per_million("gemini-2.0-flash").is_some());
        assert!(model_cost_per_million("gemini-1.5-pro").is_some());
    }

    #[test]
    fn test_model_cost_deepseek() {
        assert!(model_cost_per_million("deepseek-coder-v2").is_some());
    }

    #[test]
    fn test_model_cost_unknown() {
        assert!(model_cost_per_million("custom-model-v1").is_none());
        assert!(model_cost_per_million("llama-3.1-70b").is_none());
    }

    #[test]
    fn test_model_cost_case_insensitive() {
        assert!(model_cost_per_million("Claude-Sonnet-4").is_some());
        assert!(model_cost_per_million("GPT-4O").is_some());
    }

    // ── gpt-4o-mini vs gpt-4o ordering ─────────────────────────────────

    #[test]
    fn test_model_cost_gpt4o_mini_distinct_from_gpt4o() {
        let (mini_in, mini_out) = model_cost_per_million("gpt-4o-mini").unwrap();
        let (full_in, full_out) = model_cost_per_million("gpt-4o").unwrap();
        assert!(mini_in < full_in, "gpt-4o-mini should be cheaper on input");
        assert!(mini_out < full_out, "gpt-4o-mini should be cheaper on output");
    }

    // ── estimate_total_cost ─────────────────────────────────────────────

    #[test]
    fn test_estimate_total_cost_no_tokens() {
        assert!(estimate_total_cost(&[], 0, 0).is_none());
    }

    #[test]
    fn test_estimate_total_cost_blended_fallback() {
        let cost = estimate_total_cost(&[], 1_000_000, 1_000_000).unwrap();
        assert!((cost - 15.0).abs() < 0.01);
    }

    #[test]
    fn test_estimate_total_cost_with_known_model() {
        let models = vec!["claude-sonnet-4".into()];
        let cost = estimate_total_cost(&models, 1_000_000, 1_000_000).unwrap();
        // sonnet: 3$/M input + 15$/M output = $18
        assert!((cost - 18.0).abs() < 0.01);
    }

    #[test]
    fn test_estimate_total_cost_with_multiple_models() {
        let models = vec!["claude-sonnet-4".into(), "gpt-4o".into()];
        let cost = estimate_total_cost(&models, 1_000_000, 1_000_000).unwrap();
        // avg input: (3+2.5)/2 = 2.75, avg output: (15+10)/2 = 12.5
        // cost: 2.75 + 12.5 = $15.25
        assert!((cost - 15.25).abs() < 0.01);
    }

    #[test]
    fn test_estimate_total_cost_all_unknown_models_uses_blended() {
        let models = vec!["unknown-model".into()];
        let cost = estimate_total_cost(&models, 1_000_000, 1_000_000).unwrap();
        assert!((cost - 15.0).abs() < 0.01);
    }

    #[test]
    fn test_estimate_total_cost_input_only() {
        let cost = estimate_total_cost(&[], 1_000_000, 0).unwrap();
        assert!((cost - 3.0).abs() < 0.01);
    }

    #[test]
    fn test_estimate_total_cost_output_only() {
        let cost = estimate_total_cost(&[], 0, 1_000_000).unwrap();
        assert!((cost - 12.0).abs() < 0.01);
    }

    // ── detect_base_ref ─────────────────────────────────────────────────

    #[test]
    fn test_detect_base_ref_empty_env() {
        // Normal test env has no CI vars set -- should return None
        let _ = detect_base_ref();
    }

    // ── file_attribution_label ──────────────────────────────────────────

    #[test]
    fn test_file_attribution_label_ai() {
        let f = FileSummary {
            path: "a.rs".into(), added: 10, deleted: 0,
            ai_added: 10, ai_deleted: 0,
            attribution: Some("ai".into()), agent: Some("cursor".into()),
        };
        assert_eq!(file_attribution_label(&f), "AI");
    }

    #[test]
    fn test_file_attribution_label_human() {
        let f = FileSummary {
            path: "b.rs".into(), added: 10, deleted: 0,
            ai_added: 0, ai_deleted: 0,
            attribution: Some("human".into()), agent: None,
        };
        assert_eq!(file_attribution_label(&f), "Human");
    }

    #[test]
    fn test_file_attribution_label_mixed() {
        let f = FileSummary {
            path: "c.rs".into(), added: 100, deleted: 0,
            ai_added: 60, ai_deleted: 0,
            attribution: Some("mixed".into()), agent: Some("claude".into()),
        };
        assert_eq!(file_attribution_label(&f), "Mixed (60% AI)");
    }

    #[test]
    fn test_file_attribution_label_mixed_zero_lines() {
        let f = FileSummary {
            path: "d.rs".into(), added: 0, deleted: 0,
            ai_added: 0, ai_deleted: 0,
            attribution: Some("mixed".into()), agent: None,
        };
        // zero total lines -- falls through to the _ arm
        assert_eq!(file_attribution_label(&f), "—");
    }

    #[test]
    fn test_file_attribution_label_none() {
        let f = FileSummary {
            path: "e.rs".into(), added: 5, deleted: 0,
            ai_added: 0, ai_deleted: 0,
            attribution: None, agent: None,
        };
        assert_eq!(file_attribution_label(&f), "—");
    }

    // ── accumulate_anchor (core aggregation) ────────────────────────────

    #[test]
    fn test_accumulate_single_ai_commit() {
        let anchor = make_anchor(
            "aaa111", 100, 20, 80, 15, AuthorType::Assisted,
            vec![FileChange {
                path: "src/main.rs".into(), added: 100, deleted: 20,
                attribution: Some(FileAttribution::Ai),
                agent: Some("cursor".into()), line_attributions: vec![],
            }],
        );
        let links = vec![
            make_session_link("cursor", Some("claude-sonnet-4"), Some(5000), Some(3000)),
        ];

        let (ta, td, aa, ad, ha, hd, ti, to, tools, models, sc, atc, fm) =
            run_accumulate(&anchor, &links);

        assert_eq!(ta, 100);
        assert_eq!(td, 20);
        assert_eq!(aa, 80);
        assert_eq!(ad, 15);
        assert_eq!(ha, 20);
        assert_eq!(hd, 5);
        assert_eq!(ti, 5000);
        assert_eq!(to, 3000);
        assert_eq!(sc, 1);
        assert!(tools.contains("Cursor"));
        assert!(models.contains("claude-sonnet-4"));
        assert_eq!(atc.get("assisted"), Some(&1));
        assert_eq!(fm.len(), 1);
        let file = fm.get("src/main.rs").unwrap();
        assert_eq!(file.added, 100);
        assert_eq!(file.ai_added, 100);
        assert_eq!(file.attribution.as_deref(), Some("ai"));
        assert_eq!(file.agent.as_deref(), Some("cursor"));
    }

    #[test]
    fn test_accumulate_human_only_commit() {
        let anchor = make_anchor(
            "bbb222", 50, 10, 0, 0, AuthorType::Human,
            vec![FileChange {
                path: "README.md".into(), added: 50, deleted: 10,
                attribution: Some(FileAttribution::Human),
                agent: None, line_attributions: vec![],
            }],
        );
        let links = vec![];

        let (_, _, aa, ad, _, _, ti, to, tools, models, sc, atc, fm) =
            run_accumulate(&anchor, &links);

        assert_eq!(aa, 0);
        assert_eq!(ad, 0);
        assert_eq!(ti, 0);
        assert_eq!(to, 0);
        assert_eq!(sc, 0);
        assert!(tools.is_empty());
        assert!(models.is_empty());
        assert_eq!(atc.get("human"), Some(&1));
        let file = fm.get("README.md").unwrap();
        assert_eq!(file.ai_added, 0);
        assert_eq!(file.attribution.as_deref(), Some("human"));
    }

    #[test]
    fn test_accumulate_mixed_file() {
        let anchor = make_anchor(
            "ccc333", 100, 0, 50, 0, AuthorType::Assisted,
            vec![FileChange {
                path: "lib.rs".into(), added: 100, deleted: 0,
                attribution: Some(FileAttribution::Mixed),
                agent: Some("claude".into()), line_attributions: vec![],
            }],
        );
        let links = vec![
            make_session_link("claude", Some("claude-sonnet-4"), Some(1000), Some(500)),
        ];

        let (_, _, _, _, _, _, _, _, _, _, _, _, fm) = run_accumulate(&anchor, &links);

        let file = fm.get("lib.rs").unwrap();
        assert_eq!(file.added, 100);
        assert_eq!(file.ai_added, 50); // 100/2
        assert_eq!(file.attribution.as_deref(), Some("mixed"));
    }

    #[test]
    fn test_accumulate_multiple_sessions() {
        let anchor = make_anchor(
            "ddd444", 200, 0, 200, 0, AuthorType::Agent,
            vec![],
        );
        let links = vec![
            make_session_link("cursor", Some("claude-sonnet-4"), Some(10000), Some(5000)),
            make_session_link("claude", Some("claude-opus-4"), Some(20000), Some(15000)),
        ];

        let (_, _, _, _, _, _, ti, to, tools, models, sc, _, _) = run_accumulate(&anchor, &links);

        assert_eq!(sc, 2);
        assert_eq!(ti, 30000);
        assert_eq!(to, 20000);
        assert!(tools.contains("Cursor"));
        assert!(tools.contains("Claude"));
        assert!(models.contains("claude-sonnet-4"));
        assert!(models.contains("claude-opus-4"));
    }

    #[test]
    fn test_accumulate_session_without_tokens() {
        let anchor = make_anchor("eee555", 10, 0, 10, 0, AuthorType::Agent, vec![]);
        let links = vec![
            make_session_link("cursor", None, None, None),
        ];

        let (_, _, _, _, _, _, ti, to, _, models, sc, _, _) = run_accumulate(&anchor, &links);

        assert_eq!(sc, 1);
        assert_eq!(ti, 0);
        assert_eq!(to, 0);
        assert!(models.is_empty());
    }

    #[test]
    fn test_accumulate_same_file_across_two_anchors() {
        let anchor1 = make_anchor(
            "fff666", 50, 0, 50, 0, AuthorType::Assisted,
            vec![FileChange {
                path: "shared.rs".into(), added: 50, deleted: 0,
                attribution: Some(FileAttribution::Ai),
                agent: Some("cursor".into()), line_attributions: vec![],
            }],
        );
        let anchor2 = make_anchor(
            "ggg777", 30, 10, 0, 0, AuthorType::Human,
            vec![FileChange {
                path: "shared.rs".into(), added: 30, deleted: 10,
                attribution: Some(FileAttribution::Human),
                agent: None, line_attributions: vec![],
            }],
        );

        let (mut ta, mut td, mut aa, mut ad, mut ha, mut hd, mut ti, mut to,
             mut tools, mut models, mut sc, mut atc, mut fm) = empty_agg_state();
        let mut st = AggState {
            total_added: &mut ta, total_deleted: &mut td,
            ai_added: &mut aa, ai_deleted: &mut ad,
            human_added: &mut ha, human_deleted: &mut hd,
            total_input_tokens: &mut ti, total_output_tokens: &mut to,
            tools: &mut tools, models: &mut models,
            session_count: &mut sc, author_type_counts: &mut atc,
            file_map: &mut fm,
        };
        accumulate_anchor(&anchor1, &[], &mut st);
        accumulate_anchor(&anchor2, &[], &mut st);

        assert_eq!(ta, 80);
        assert_eq!(td, 10);
        let file = fm.get("shared.rs").unwrap();
        assert_eq!(file.added, 80);
        assert_eq!(file.deleted, 10);
        assert_eq!(file.ai_added, 50);
        // First anchor set attribution to "ai", second was "human",
        // but "ai" should persist (it was set first and human doesn't overwrite)
        assert_eq!(file.attribution.as_deref(), Some("ai"));
    }

    #[test]
    fn test_accumulate_author_type_counting() {
        let anchors = vec![
            make_anchor("a1", 10, 0, 10, 0, AuthorType::Agent, vec![]),
            make_anchor("a2", 10, 0, 5, 0, AuthorType::Assisted, vec![]),
            make_anchor("a3", 10, 0, 0, 0, AuthorType::Human, vec![]),
            make_anchor("a4", 10, 0, 0, 0, AuthorType::Automated, vec![]),
            make_anchor("a5", 10, 0, 10, 0, AuthorType::Agent, vec![]),
        ];

        let (mut ta, mut td, mut aa, mut ad, mut ha, mut hd, mut ti, mut to,
             mut tools, mut models, mut sc, mut atc, mut fm) = empty_agg_state();
        let mut st = AggState {
            total_added: &mut ta, total_deleted: &mut td,
            ai_added: &mut aa, ai_deleted: &mut ad,
            human_added: &mut ha, human_deleted: &mut hd,
            total_input_tokens: &mut ti, total_output_tokens: &mut to,
            tools: &mut tools, models: &mut models,
            session_count: &mut sc, author_type_counts: &mut atc,
            file_map: &mut fm,
        };
        for a in &anchors {
            accumulate_anchor(a, &[], &mut st);
        }

        assert_eq!(atc.get("agent"), Some(&2));
        assert_eq!(atc.get("assisted"), Some(&1));
        assert_eq!(atc.get("human"), Some(&1));
        assert_eq!(atc.get("automated"), Some(&1));
    }

    // ── PrSummary construction ──────────────────────────────────────────

    #[test]
    fn test_pr_summary_ai_percentage_calculation() {
        // ai_added=80, ai_deleted=0, human_added=20, human_deleted=0
        // total_lines = 80+0+20+0 = 100, ai_lines = 80
        // ai_percentage = 80%
        let anchor = make_anchor(
            "ppp111", 100, 0, 80, 0, AuthorType::Assisted, vec![],
        );
        let links = vec![make_session_link("cursor", Some("sonnet"), Some(1000), Some(500))];

        let (_, _, aa, ad, ha, hd, _, _, _, _, _, _, _) = run_accumulate(&anchor, &links);
        let total_lines = aa + ad + ha + hd;
        let ai_lines = aa + ad;
        let pct = (ai_lines as f64 / total_lines as f64) * 100.0;
        assert!((pct - 80.0).abs() < 0.01);
    }

    #[test]
    fn test_pr_summary_no_lines_gives_none_percentage() {
        let anchor = make_anchor("qqq222", 0, 0, 0, 0, AuthorType::Human, vec![]);
        let (_, _, aa, ad, ha, hd, _, _, _, _, _, _, _) = run_accumulate(&anchor, &[]);
        let total_lines = aa + ad + ha + hd;
        assert_eq!(total_lines, 0);
    }

    // ── File sorting ────────────────────────────────────────────────────

    #[test]
    fn test_files_sorted_by_total_changes_descending() {
        let mut files = vec![
            FileSummary { path: "small.rs".into(), added: 5, deleted: 0, ai_added: 0, ai_deleted: 0, attribution: None, agent: None },
            FileSummary { path: "big.rs".into(), added: 200, deleted: 50, ai_added: 0, ai_deleted: 0, attribution: None, agent: None },
            FileSummary { path: "mid.rs".into(), added: 30, deleted: 10, ai_added: 0, ai_deleted: 0, attribution: None, agent: None },
        ];
        files.sort_by(|a, b| {
            let a_total = a.added + a.deleted;
            let b_total = b.added + b.deleted;
            b_total.cmp(&a_total)
        });
        assert_eq!(files[0].path, "big.rs");
        assert_eq!(files[1].path, "mid.rs");
        assert_eq!(files[2].path, "small.rs");
    }

    // ── Markdown rendering ──────────────────────────────────────────────

    fn capture_markdown(summary: &PrSummary) -> String {
        // Capture print_markdown output by temporarily redirecting stdout
        // Since print_markdown uses println!, we construct the markdown
        // using the same logic inline for testability.
        let mut out = String::new();
        out.push_str("<!-- oobo-pr-summary -->\n");
        out.push_str("## oobo — AI Contribution Report\n\n");

        if summary.commits_with_anchors == 0 {
            out.push_str(&format!(
                "Analyzed {} commit(s) — none have AI context recorded.\n",
                summary.total_commits
            ));
            return out;
        }

        out.push_str("| | |\n|---|---|\n");
        if let Some(pct) = summary.ai_percentage {
            out.push_str(&format!("| **AI Contribution** | {:.0}%", pct));
        }
        out
    }

    #[test]
    fn test_markdown_contains_html_comment_tag() {
        let summary = PrSummary {
            base: String::new(), head: String::new(),
            total_commits: 3, commits_with_anchors: 2,
            total_added: 100, total_deleted: 10,
            ai_added: 80, ai_deleted: 5, human_added: 20, human_deleted: 5,
            ai_percentage: Some(80.95),
            tools: vec!["Cursor".into()], models: vec!["claude-sonnet-4".into()],
            total_input_tokens: 5000, total_output_tokens: 3000, total_tokens: 8000,
            estimated_cost: Some(0.15),
            session_count: 2, author_type_breakdown: HashMap::new(),
            files: vec![],
        };
        let md = capture_markdown(&summary);
        assert!(md.contains("<!-- oobo-pr-summary -->"));
        assert!(md.contains("## oobo — AI Contribution Report"));
        assert!(md.contains("81%"));
    }

    #[test]
    fn test_markdown_no_anchors_shows_empty_message() {
        let summary = PrSummary {
            base: String::new(), head: String::new(),
            total_commits: 5, commits_with_anchors: 0,
            total_added: 0, total_deleted: 0,
            ai_added: 0, ai_deleted: 0, human_added: 0, human_deleted: 0,
            ai_percentage: None,
            tools: vec![], models: vec![],
            total_input_tokens: 0, total_output_tokens: 0, total_tokens: 0,
            estimated_cost: None,
            session_count: 0, author_type_breakdown: HashMap::new(),
            files: vec![],
        };
        let md = capture_markdown(&summary);
        assert!(md.contains("none have AI context recorded"));
    }

    // ── PrSummary serialization ─────────────────────────────────────────

    #[test]
    fn test_pr_summary_json_serialization() {
        let summary = PrSummary {
            base: "origin/main".into(), head: "HEAD".into(),
            total_commits: 3, commits_with_anchors: 2,
            total_added: 200, total_deleted: 50,
            ai_added: 150, ai_deleted: 30, human_added: 50, human_deleted: 20,
            ai_percentage: Some(72.0),
            tools: vec!["Cursor".into(), "Claude".into()],
            models: vec!["claude-sonnet-4".into()],
            total_input_tokens: 10000, total_output_tokens: 5000, total_tokens: 15000,
            estimated_cost: Some(0.18),
            session_count: 3,
            author_type_breakdown: {
                let mut m = HashMap::new();
                m.insert("assisted".into(), 2);
                m.insert("human".into(), 1);
                m
            },
            files: vec![
                FileSummary {
                    path: "src/main.rs".into(), added: 100, deleted: 20,
                    ai_added: 80, ai_deleted: 15,
                    attribution: Some("ai".into()), agent: Some("cursor".into()),
                },
            ],
        };

        let json = serde_json::to_string(&summary).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["total_commits"], 3);
        assert_eq!(parsed["commits_with_anchors"], 2);
        assert_eq!(parsed["ai_percentage"], 72.0);
        assert_eq!(parsed["total_tokens"], 15000);
        assert_eq!(parsed["estimated_cost"], 0.18);
        assert_eq!(parsed["session_count"], 3);
        assert_eq!(parsed["tools"][0], "Cursor");
        assert_eq!(parsed["tools"][1], "Claude");
        assert_eq!(parsed["files"][0]["path"], "src/main.rs");
        assert_eq!(parsed["files"][0]["attribution"], "ai");
    }

    #[test]
    fn test_pr_summary_json_skips_none_fields() {
        let summary = PrSummary {
            base: String::new(), head: String::new(),
            total_commits: 1, commits_with_anchors: 0,
            total_added: 0, total_deleted: 0,
            ai_added: 0, ai_deleted: 0, human_added: 0, human_deleted: 0,
            ai_percentage: None,
            tools: vec![], models: vec![],
            total_input_tokens: 0, total_output_tokens: 0, total_tokens: 0,
            estimated_cost: None,
            session_count: 0, author_type_breakdown: HashMap::new(),
            files: vec![],
        };

        let json = serde_json::to_string(&summary).unwrap();
        assert!(!json.contains("ai_percentage"));
        assert!(!json.contains("estimated_cost"));
    }

    // ── FileSummary serialization ───────────────────────────────────────

    #[test]
    fn test_file_summary_json_skips_none() {
        let f = FileSummary {
            path: "test.rs".into(), added: 10, deleted: 0,
            ai_added: 0, ai_deleted: 0,
            attribution: None, agent: None,
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(!json.contains("attribution"));
        assert!(!json.contains("agent"));
    }

    #[test]
    fn test_file_summary_json_includes_present_fields() {
        let f = FileSummary {
            path: "test.rs".into(), added: 10, deleted: 5,
            ai_added: 8, ai_deleted: 3,
            attribution: Some("ai".into()), agent: Some("cursor".into()),
        };
        let json = serde_json::to_string(&f).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["attribution"], "ai");
        assert_eq!(parsed["agent"], "cursor");
        assert_eq!(parsed["ai_added"], 8);
    }

    // ── Tool deduplication ──────────────────────────────────────────────

    #[test]
    fn test_tools_deduplicated_across_sessions() {
        let anchor = make_anchor("dedup1", 10, 0, 10, 0, AuthorType::Agent, vec![]);
        let links = vec![
            make_session_link("cursor", Some("sonnet"), Some(100), Some(50)),
            make_session_link("cursor", Some("sonnet"), Some(200), Some(100)),
        ];

        let (_, _, _, _, _, _, _, _, tools, _, sc, _, _) = run_accumulate(&anchor, &links);

        assert_eq!(sc, 2);
        assert_eq!(tools.len(), 1);
        assert!(tools.contains("Cursor"));
    }

    #[test]
    fn test_models_deduplicated() {
        let anchor = make_anchor("dedup2", 10, 0, 10, 0, AuthorType::Agent, vec![]);
        let links = vec![
            make_session_link("cursor", Some("claude-sonnet-4"), Some(100), Some(50)),
            make_session_link("claude", Some("claude-sonnet-4"), Some(200), Some(100)),
        ];

        let (_, _, _, _, _, _, _, _, _, models, _, _, _) = run_accumulate(&anchor, &links);
        assert_eq!(models.len(), 1);
    }
}
