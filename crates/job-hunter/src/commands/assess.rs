use super::{ensure_score, job_label, Ctx, Heartbeat};
use anyhow::{bail, Result};
use clap::Args;
use job_hunter_core::agent::AgentRunner;
use job_hunter_core::assessment::prompt;
use job_hunter_core::assessment::schema::{AssessmentFacts, CURRENT_SCHEMA_VERSION};
use job_hunter_core::db::{AssessmentKey, JobFilter, JobStatus, NewAssessment};
use job_hunter_core::profile::Profile;
use job_hunter_core::workspace::AgentTask;
use job_hunter_core::{feedback, normalize_identity, sha256_hex};
use std::io::Write;
use std::time::{Duration, Instant};

#[derive(Args)]
pub struct AssessArgs {
    /// Specific job ids (default: every pending job)
    pub ids: Vec<i64>,
    /// Assess every pending job (same as passing no ids; kept for clarity)
    #[arg(long)]
    pub all: bool,
    /// Re-assess even when a current assessment exists
    #[arg(long)]
    pub force: bool,
}

/// CV text embedded in prompts: verbatim when small (lossless), otherwise a
/// cached one-off distillation keyed by CV hash. The cache file is plain text
/// the user can review and hand-edit.
fn cv_for_prompt(ctx: &Ctx, runner: &AgentRunner, cv_text: &str, cv_hash: &str) -> Result<String> {
    if cv_text.chars().count() <= ctx.config.assessment.cv_verbatim_max_chars {
        return Ok(cv_text.to_string());
    }
    let cache = ctx.paths.cv_digest_dir.join(format!("{cv_hash}.txt"));
    if let Ok(cached) = std::fs::read_to_string(&cache) {
        if !cached.trim().is_empty() {
            return Ok(cached.trim().to_string());
        }
    }
    println!(
        "CV exceeds {} chars - distilling once via the agent (cached at {})...",
        ctx.config.assessment.cv_verbatim_max_chars,
        cache.display()
    );
    let digest = runner.run_text(&prompt::build_cv_digest_prompt(
        cv_text,
        ctx.config.assessment.cv_digest_max_words,
    ))?;
    if digest.trim().is_empty() {
        bail!("agent returned an empty CV digest");
    }
    std::fs::create_dir_all(&ctx.paths.cv_digest_dir)?;
    std::fs::write(&cache, format!("{}\n", digest.trim()))?;
    println!("CV digest cached - review and hand-edit it if anything important was dropped.");
    Ok(digest.trim().to_string())
}

/// ", +background +preferences" - names the supplementary documents in play,
/// so a prompt that grew is visible in the run header rather than only in the
/// re-assessment it triggers.
pub fn profile_summary(profile: &Profile) -> String {
    let mut parts = Vec::new();
    if profile.background.is_some() {
        parts.push("+background");
    }
    if profile.preferences.is_some() {
        parts.push("+preferences");
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(", profile {}", parts.join(" "))
    }
}

pub fn run(args: AssessArgs) -> Result<()> {
    run_with_ctx(&Ctx::load()?, args)
}

pub fn run_with_ctx(ctx: &Ctx, args: AssessArgs) -> Result<()> {
    let criteria_hash = ctx.config.criteria_hash();
    let prompt_version = ctx.config.assessment.prompt_version;

    let profile = Profile::load(&ctx.config.resources)?;
    let supplement_hash = profile.supplement_hash();

    let workspace = ctx.agent_workspace()?;
    let instructions_hash = workspace.instructions_hash(AgentTask::Assess)?;
    let key = AssessmentKey {
        cv_hash: &profile.cv_hash,
        profile_hash: supplement_hash.as_deref(),
        prompt_version,
        instructions_hash: &instructions_hash,
    };

    // Which jobs need the LLM?
    let mut targets = Vec::new();
    if args.ids.is_empty() {
        let candidates = ctx.db.list_jobs(&JobFilter {
            statuses: Some(vec![JobStatus::New, JobStatus::Assessed]),
            include_duplicates: false,
        })?;
        for job in candidates {
            if args.force || !ctx.db.assessment_exists(job.id, &key)? {
                targets.push(job);
            }
        }
    } else {
        for id in &args.ids {
            let job = ctx.db.get_job(*id)?;
            if let Some(canonical) = job.duplicate_of {
                println!("job #{id} is a duplicate of #{canonical} - skipping (assess #{canonical} instead)");
                continue;
            }
            if !args.force && ctx.db.assessment_exists(job.id, &key)? {
                println!("job #{id} already assessed under the current profile/prompt/instructions - use --force to re-run");
                continue;
            }
            targets.push(job);
        }
    }
    if targets.is_empty() {
        println!("Nothing to assess.");
        return Ok(());
    }

    let runner = AgentRunner::new(&ctx.config.agent).with_workspace(workspace.dir.clone());
    let cv_prompt_text = cv_for_prompt(ctx, &runner, &profile.cv_text, &profile.cv_hash)?;
    let profile_prompt =
        profile.for_prompt(cv_prompt_text, ctx.config.assessment.max_profile_chars);
    let feedback_digest = feedback::build_prompt_digest(&ctx.db, ctx.config.assessment.feedback_recent_n)?;
    let feedback_digest_hash = feedback_digest.as_deref().map(|d| sha256_hex(d.as_bytes())[..16].to_string());
    let agent_command = ctx.config.agent.command.join(" ");

    let total = targets.len();
    println!(
        "Assessing {total} job(s) with `{agent_command}` (workspace {}, instructions {instructions_hash}{})...\n",
        workspace.dir.display(),
        profile_summary(&profile),
    );
    let overall_start = Instant::now();
    let mut failures = 0usize;
    for (i, job) in targets.iter().enumerate() {
        let assessment_prompt = prompt::build_assessment_prompt(
            &profile_prompt,
            feedback_digest.as_deref(),
            &job.content_text,
            ctx.config.assessment.max_job_chars,
        );
        print!("[{}/{total}] job #{} ({})... ", i + 1, job.id, job_label(job));
        std::io::stdout().flush().ok();

        let heartbeat = Heartbeat::start(Duration::from_secs(5));
        let started = Instant::now();
        let result = runner.run_json::<AssessmentFacts>(&assessment_prompt);
        heartbeat.stop();
        let elapsed = started.elapsed().as_secs_f64();

        let (facts, facts_json) = match result {
            Ok(ok) => ok,
            Err(e) => {
                println!(" FAILED ({elapsed:.1}s): {e:#}");
                failures += 1;
                continue;
            }
        };

        ctx.db.upsert_assessment(&NewAssessment {
            job_id: job.id,
            cv_hash: &profile.cv_hash,
            profile_hash: supplement_hash.as_deref(),
            prompt_version,
            schema_version: if facts.schema_version > 0 {
                facts.schema_version
            } else {
                CURRENT_SCHEMA_VERSION
            },
            feedback_digest_hash: feedback_digest_hash.as_deref(),
            instructions_hash: Some(&instructions_hash),
            agent_command: &agent_command,
            facts_json: &facts_json,
        })?;

        // Backfill LLM-extracted identity; link title+company duplicates.
        let title_norm = facts.extracted.job_title.as_deref().and_then(normalize_identity);
        let company_norm = facts.extracted.company.as_deref().and_then(normalize_identity);
        let duplicate_of = match (&title_norm, &company_norm) {
            (Some(t), Some(c)) => ctx.db.find_title_company_match(t, c, job.id)?,
            _ => None,
        };
        ctx.db.backfill_extraction(
            job.id,
            facts.extracted.job_title.as_deref(),
            facts.extracted.company.as_deref(),
            facts.extracted.location.as_deref(),
            title_norm.as_deref(),
            company_norm.as_deref(),
            duplicate_of,
        )?;
        if job.status == JobStatus::New.as_str() {
            ctx.db.set_status(job.id, JobStatus::Assessed, None)?;
        }

        let scored = ensure_score(ctx, job.id, &criteria_hash)?;
        match scored {
            Some((_, score)) => {
                let mark = if score.hard_filter_passed { "✓" } else { "✗ filtered" };
                print!(" score {:.1} {mark} ({elapsed:.1}s)", score.score);
            }
            None => print!(" assessed ({elapsed:.1}s)"),
        }
        if let Some(dup) = duplicate_of {
            print!("  (duplicate of job #{dup})");
        }
        println!();
    }

    if failures > 0 {
        bail!("{failures} of {} assessment(s) failed", targets.len());
    }
    println!("\nDone in {:.0}s. Run `job-hunter jobs` to see the ranking.", overall_start.elapsed().as_secs_f64());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::{
        fixtures_dir, load_payload, mock_agent, test_ctx, test_ctx_with_resources,
    };
    use job_hunter_core::ingest::ingest_payload;

    fn assess_all(ctx: &Ctx) -> Result<()> {
        run_with_ctx(ctx, AssessArgs { ids: vec![], all: true, force: false })
    }

    #[test]
    fn full_pipeline_assesses_scores_and_links_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path(), &mock_agent("mock-agent.sh", None));

        let first = ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();
        let second =
            ingest_payload(&ctx.db, &load_payload("sample2-same-role-other-board.json")).unwrap();
        assert_ne!(first.id, second.id, "different URLs are separate rows at save time");

        assess_all(&ctx).unwrap();

        // Both assessed; extraction backfilled; same title+company -> linked.
        let job1 = ctx.db.get_job(first.id).unwrap();
        assert_eq!(job1.title.as_deref(), Some("Senior Rust Engineer"));
        assert_eq!(job1.company.as_deref(), Some("Acme Ltd"));
        assert_eq!(job1.status, "assessed");
        assert_eq!(job1.duplicate_of, None);
        let job2 = ctx.db.get_job(second.id).unwrap();
        assert_eq!(job2.duplicate_of, Some(first.id));

        // Score cached under the current criteria hash, filters passed.
        let criteria_hash = ctx.config.criteria_hash();
        let (_, score) = ensure_score(&ctx, first.id, &criteria_hash).unwrap().unwrap();
        assert!(score.hard_filter_passed);
        let expected = 100.0 * (5.0 * 0.82 + 3.0 * 1.0 + 3.0 * (110.0 / 120.0) + 1.0 * 0.6) / 12.0;
        assert!((score.score - expected).abs() < 1e-6, "got {}", score.score);

        // Re-running assess finds nothing new to do (no assessment rows change).
        assess_all(&ctx).unwrap();
    }

    #[test]
    fn criteria_change_rescores_without_agent() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path(), &mock_agent("mock-agent.sh", None));
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();
        assess_all(&ctx).unwrap();
        let hash_before = ctx.config.criteria_hash();
        let (_, before) = ensure_score(&ctx, 1, &hash_before).unwrap().unwrap();

        // Tighten the salary filter; swap the agent for one that always fails
        // to prove rescoring never invokes it.
        ctx.config.criteria.hard_filters[0].value = serde_json::json!(150000);
        ctx.config.agent.command = mock_agent("mock-agent-fail.sh", None);
        let hash_after = ctx.config.criteria_hash();
        assert_ne!(hash_before, hash_after);

        let (_, after) = ensure_score(&ctx, 1, &hash_after).unwrap().unwrap();
        assert!(!after.hard_filter_passed, "110k < 150k threshold must fail");
        assert_eq!(before.score, after.score, "soft score unchanged by filter edit");
    }

    #[test]
    fn flaky_agent_is_retried() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("flaky-count");
        let script = fixtures_dir().join("mock-agent-flaky.sh");
        let command = vec![
            "env".to_string(),
            format!("MOCK_FLAKY_STATE={}", state.display()),
            script.to_string_lossy().into_owned(),
        ];
        let ctx = test_ctx(dir.path(), &command);
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();
        assess_all(&ctx).unwrap();
        assert_eq!(std::fs::read_to_string(&state).unwrap().trim(), "2", "first call failed, retry succeeded");
        assert!(ctx.db.latest_assessment(1).unwrap().is_some());
    }

    #[test]
    fn failing_agent_reports_and_leaves_job_unassessed() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path(), &mock_agent("mock-agent-fail.sh", None));
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();
        let err = assess_all(&ctx).unwrap_err();
        assert!(format!("{err}").contains("1 of 1"));
        assert!(ctx.db.latest_assessment(1).unwrap().is_none());
        assert_eq!(ctx.db.get_job(1).unwrap().status, "new");
    }

    #[test]
    fn editing_instructions_reassesses_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("captured-prompt");
        let ctx = test_ctx(dir.path(), &mock_agent("mock-agent.sh", Some(&capture)));
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();

        assess_all(&ctx).unwrap();
        let first_hash = ctx.db.latest_assessment(1).unwrap().unwrap().instructions_hash;
        assert!(first_hash.is_some(), "assessments store the instructions hash");

        // Edit the assess skill: the next plain `assess` must re-run the job.
        let skill = dir.path().join("agent").join(AgentTask::Assess.skill_path());
        let mut text = std::fs::read_to_string(&skill).unwrap();
        text.push_str("\n- Also flag on-call expectations.\n");
        std::fs::write(&skill, text).unwrap();
        assess_all(&ctx).unwrap();

        let second_hash = ctx.db.latest_assessment(1).unwrap().unwrap().instructions_hash;
        assert_ne!(first_hash, second_hash);
        let prompts = std::fs::read_to_string(&capture).unwrap();
        assert_eq!(prompts.matches("\n----\n").count(), 2, "two agent calls so far");

        // Unchanged instructions: nothing to do, no third call.
        assess_all(&ctx).unwrap();
        let prompts = std::fs::read_to_string(&capture).unwrap();
        assert_eq!(prompts.matches("\n----\n").count(), 2);
    }

    #[test]
    fn prompt_is_pointer_plus_data_and_workspace_is_materialized() {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("captured-prompt");
        let ctx = test_ctx(dir.path(), &mock_agent("mock-agent.sh", Some(&capture)));
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();
        assess_all(&ctx).unwrap();

        // The workspace was auto-created next to the test config.
        let ws = dir.path().join("agent");
        assert!(ws.join("AGENTS.md").exists());
        assert!(ws.join("CLAUDE.md").exists());
        assert!(ws.join(AgentTask::Assess.skill_path()).exists());

        // The prompt carries the pointer, contract, and data - not the rules
        // prose, which lives in the skill file. (The mock ignores its cwd
        // entirely, proving mock backends need no workspace awareness.)
        let prompt = std::fs::read_to_string(&capture).unwrap();
        assert!(prompt.contains(".claude/skills/assess/SKILL.md"));
        assert!(prompt.contains("schema_version"));
        assert!(prompt.contains("Test Candidate"), "CV embedded");
        assert!(prompt.contains("Senior Rust Engineer"), "job text embedded");
        assert!(!prompt.contains("never collapse a stated range"));
    }

    #[test]
    fn legacy_null_hash_rows_are_not_rerun() {
        let dir = tempfile::tempdir().unwrap();
        // A failing agent proves no LLM call happens for grandfathered rows.
        let ctx = test_ctx(dir.path(), &mock_agent("mock-agent-fail.sh", None));
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();

        let cv_text = std::fs::read_to_string(fixtures_dir().join("sample-cv.md")).unwrap();
        let cv_hash = job_hunter_core::sha256_hex(cv_text.as_bytes());
        ctx.db
            .upsert_assessment(&NewAssessment {
                job_id: 1,
                cv_hash: &cv_hash,
                profile_hash: None,
                prompt_version: 1,
                schema_version: 1,
                feedback_digest_hash: None,
                instructions_hash: None,
                agent_command: "old-binary",
                facts_json: r#"{"schema_version":1}"#,
            })
            .unwrap();

        assess_all(&ctx).unwrap();
        assert!(ctx.db.latest_assessment(1).unwrap().unwrap().instructions_hash.is_none());
    }

    #[test]
    fn supplementary_documents_reach_the_prompt_and_rerun_on_edit() {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("captured-prompt");
        let command = mock_agent("mock-agent.sh", Some(&capture));
        let background = dir.path().join("background.md");
        let preferences = dir.path().join("preferences.md");
        std::fs::write(&background, "Rewrote the payments ledger end to end.").unwrap();
        std::fs::write(&preferences, "Remote only. No on-call rota.").unwrap();
        let resources = format!(
            "  background_path: {}\n  preferences_path: {}",
            background.display(),
            preferences.display()
        );
        let ctx = test_ctx_with_resources(dir.path(), &command, &resources);
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();

        assess_all(&ctx).unwrap();
        let prompt = std::fs::read_to_string(&capture).unwrap();
        assert!(prompt.contains("Rewrote the payments ledger end to end."));
        assert!(prompt.contains("No on-call rota."));
        let first = ctx.db.latest_assessment(1).unwrap().unwrap().profile_hash;
        assert!(first.is_some(), "the supplements are hashed into the assessment");

        // Unchanged profile: nothing to do.
        assess_all(&ctx).unwrap();
        let prompt = std::fs::read_to_string(&capture).unwrap();
        assert_eq!(prompt.matches("\n----\n").count(), 1);

        // Editing a supplement re-runs, exactly like editing the CV.
        std::fs::write(&preferences, "Remote only. On-call is fine.").unwrap();
        assess_all(&ctx).unwrap();
        let prompt = std::fs::read_to_string(&capture).unwrap();
        assert_eq!(prompt.matches("\n----\n").count(), 2);
        assert_ne!(ctx.db.latest_assessment(1).unwrap().unwrap().profile_hash, first);
    }

    #[test]
    fn a_profile_with_no_supplements_is_not_rerun() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path(), &mock_agent("mock-agent.sh", None));
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();
        assess_all(&ctx).unwrap();
        assert!(ctx.db.latest_assessment(1).unwrap().unwrap().profile_hash.is_none());

        // A failing agent proves the second run makes no call at all.
        let mut ctx = ctx;
        ctx.config.agent.command =
            mock_agent("mock-agent-fail.sh", None);
        assess_all(&ctx).unwrap();
    }

    #[test]
    fn feedback_digest_is_injected_into_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("captured-prompt");
        let ctx = test_ctx(dir.path(), &mock_agent("mock-agent.sh", Some(&capture)));
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();
        ctx.db.insert_feedback(None, "never rank agencies above product companies").unwrap();

        assess_all(&ctx).unwrap();
        let prompt = std::fs::read_to_string(&capture).unwrap();
        assert!(prompt.contains("never rank agencies above product companies"));
        assert!(prompt.contains("Test Candidate"), "CV embedded verbatim");
        assert!(prompt.contains("Senior Rust Engineer"), "job text embedded");
    }
}
