//! `cv-advice`: ask the agent how to restructure the CV for one posting.
//!
//! Deliberately separate from `assess`. Assessment answers "is this job worth
//! applying to", is run over batches, and its facts are what scoring consumes.
//! This answers "how do I present myself to this one", costs a call per job,
//! and produces prose for a human - so you pay for it only on the jobs you
//! have decided to pursue.

use super::{job_label, Ctx, Heartbeat};
use anyhow::{bail, Result};
use clap::Args;
use job_hunter_core::advice::prompt as advice_prompt;
use job_hunter_core::advice::schema::{CvAdvice, CURRENT_ADVICE_SCHEMA_VERSION};
use job_hunter_core::agent::AgentRunner;
use job_hunter_core::db::{Assessment, Job, JobFilter, NewCvAdvice};
use job_hunter_core::profile::Profile;
use job_hunter_core::workspace::AgentTask;
use std::io::Write;
use std::time::{Duration, Instant};

#[derive(Args)]
pub struct CvAdviceArgs {
    /// Job ids to advise on (each must already be assessed)
    pub ids: Vec<i64>,
    /// Every assessed job that has no current advice (spends a call per job)
    #[arg(long)]
    pub all: bool,
    /// Regenerate even when current advice exists
    #[arg(long)]
    pub force: bool,
}

pub fn run(args: CvAdviceArgs) -> Result<()> {
    run_with_ctx(&Ctx::load()?, args)
}

pub fn run_with_ctx(ctx: &Ctx, args: CvAdviceArgs) -> Result<()> {
    // No implicit fan-out: unlike `assess`, an empty id list means "you forgot
    // to say which job", not "every job I own".
    if args.ids.is_empty() && !args.all {
        bail!(
            "usage: job-hunter cv-advice <job-id>...  (or --all for every assessed job \
             without current advice)"
        );
    }

    let profile = Profile::load(&ctx.config.resources)?;
    let supplement_hash = profile.supplement_hash();
    let workspace = ctx.agent_workspace()?;
    let instructions_hash = workspace.instructions_hash(AgentTask::CvAdvice)?;

    let candidates: Vec<Job> = if args.ids.is_empty() {
        ctx.db.list_jobs(&JobFilter::default())?
    } else {
        args.ids.iter().map(|id| ctx.db.get_job(*id)).collect::<Result<_>>()?
    };

    let mut targets: Vec<(Job, Assessment)> = Vec::new();
    for job in candidates {
        if let Some(canonical) = job.duplicate_of {
            println!(
                "job #{} is a duplicate of #{canonical} - skipping (advise on #{canonical} instead)",
                job.id
            );
            continue;
        }
        let Some(assessment) = ctx.db.latest_assessment(job.id)? else {
            // The assessment is an input, so there is nothing to advise from.
            if !args.ids.is_empty() {
                println!("job #{} is not assessed yet - run `job-hunter assess {}` first", job.id, job.id);
            }
            continue;
        };
        if !args.force {
            if let Some(existing) = ctx.db.cv_advice_for_assessment(assessment.id)? {
                if existing.profile_hash == supplement_hash
                    && existing.instructions_hash.as_deref() == Some(instructions_hash.as_str())
                {
                    if !args.ids.is_empty() {
                        println!(
                            "job #{} already has current advice ({}) - use --force to regenerate, \
                             `job-hunter show {}` to read it",
                            job.id, existing.created_at, job.id
                        );
                    }
                    continue;
                }
            }
        }
        targets.push((job, assessment));
    }
    if targets.is_empty() {
        println!("Nothing to advise on.");
        return Ok(());
    }

    let runner = AgentRunner::new(&ctx.config.agent).with_workspace(workspace.dir.clone());
    // The CV goes in verbatim: you cannot advise on restructuring a document
    // you were only shown a digest of.
    let profile_prompt =
        profile.for_prompt(profile.cv_text.clone(), ctx.config.assessment.max_profile_chars);
    let agent_command = ctx.config.agent.command.join(" ");

    let total = targets.len();
    println!(
        "Advising on {total} job(s) with `{agent_command}` (workspace {}, instructions {instructions_hash}{})...\n",
        workspace.dir.display(),
        super::assess::profile_summary(&profile),
    );
    let mut failures = 0usize;
    for (i, (job, assessment)) in targets.iter().enumerate() {
        let prompt = advice_prompt::build_advice_prompt(
            &profile_prompt,
            &assessment.facts_json,
            &job.content_text,
            ctx.config.assessment.max_job_chars,
        );
        print!("[{}/{total}] job #{} ({})... ", i + 1, job.id, job_label(job));
        std::io::stdout().flush().ok();

        let heartbeat = Heartbeat::start(Duration::from_secs(5));
        let started = Instant::now();
        let result = runner.run_json::<CvAdvice>(&prompt);
        heartbeat.stop();
        let elapsed = started.elapsed().as_secs_f64();

        let (advice, advice_json) = match result {
            Ok(ok) => ok,
            Err(e) => {
                println!(" FAILED ({elapsed:.1}s): {e:#}");
                failures += 1;
                continue;
            }
        };
        ctx.db.upsert_cv_advice(&NewCvAdvice {
            job_id: job.id,
            assessment_id: assessment.id,
            schema_version: if advice.schema_version > 0 {
                advice.schema_version
            } else {
                CURRENT_ADVICE_SCHEMA_VERSION
            },
            profile_hash: supplement_hash.as_deref(),
            instructions_hash: Some(&instructions_hash),
            agent_command: &agent_command,
            advice_json: &advice_json,
        })?;
        println!(
            " {} rewrite(s), {} gap(s) ({elapsed:.1}s)",
            advice.rewrites.len(),
            advice.gaps.len()
        );
    }

    if failures > 0 {
        bail!("{failures} of {total} advice call(s) failed");
    }
    println!("\nDone. Run `job-hunter show <id>` to read the advice.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::assess::{run_with_ctx as assess_with_ctx, AssessArgs};
    use crate::commands::test_support::{load_payload, mock_agent, test_ctx};
    use job_hunter_core::ingest::ingest_payload;

    fn assess_all(ctx: &Ctx) -> Result<()> {
        assess_with_ctx(ctx, AssessArgs { ids: vec![], all: true, force: false })
    }

    fn advise(ctx: &Ctx, ids: Vec<i64>) -> Result<()> {
        run_with_ctx(ctx, CvAdviceArgs { ids, all: false, force: false })
    }

    /// Prompts the mock recorded, in order.
    fn prompts(capture: &std::path::Path) -> Vec<String> {
        let text = std::fs::read_to_string(capture).unwrap_or_default();
        text.split("\n----\n").filter(|p| !p.trim().is_empty()).map(str::to_string).collect()
    }

    #[test]
    fn advice_is_generated_from_the_cv_the_facts_and_the_posting() {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("captured-prompt");
        let ctx = test_ctx(dir.path(), &mock_agent("mock-agent.sh", Some(&capture)));
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();
        assess_all(&ctx).unwrap();
        advise(&ctx, vec![1]).unwrap();

        let assessment = ctx.db.latest_assessment(1).unwrap().unwrap();
        let row = ctx.db.cv_advice_for_assessment(assessment.id).unwrap().unwrap();
        assert_eq!(row.job_id, 1);
        let advice = row.advice().unwrap();
        assert_eq!(advice.rewrites.len(), 2);
        assert!(advice.keywords.contains(&"PCI DSS".to_string()));

        // The second call is the advice one: every documented input is in it.
        let advice_prompt = prompts(&capture).remove(1);
        assert!(advice_prompt.contains("`cv-advice` skill"));
        assert!(advice_prompt.contains("Test Candidate"), "the CV, verbatim");
        assert!(advice_prompt.contains("overall_cv_match"), "the stored assessment facts");
        assert!(advice_prompt.contains("Senior Rust Engineer"), "the posting");
    }

    #[test]
    fn a_job_without_an_assessment_is_skipped_without_calling_the_agent() {
        let dir = tempfile::tempdir().unwrap();
        // A failing agent proves the guard runs before any call is made.
        let ctx = test_ctx(dir.path(), &mock_agent("mock-agent-fail.sh", None));
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();
        advise(&ctx, vec![1]).unwrap();
        assert!(ctx.db.latest_assessment(1).unwrap().is_none());
    }

    #[test]
    fn naming_no_jobs_is_a_usage_error_not_a_fan_out() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path(), &mock_agent("mock-agent.sh", None));
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();
        assess_all(&ctx).unwrap();
        let err = advise(&ctx, vec![]).unwrap_err();
        assert!(format!("{err}").contains("usage"));
        let assessment = ctx.db.latest_assessment(1).unwrap().unwrap();
        assert!(ctx.db.cv_advice_for_assessment(assessment.id).unwrap().is_none());
    }

    #[test]
    fn current_advice_is_kept_until_forced_or_invalidated() {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("captured-prompt");
        let ctx = test_ctx(dir.path(), &mock_agent("mock-agent.sh", Some(&capture)));
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();
        assess_all(&ctx).unwrap();

        advise(&ctx, vec![1]).unwrap();
        assert_eq!(prompts(&capture).len(), 2);
        advise(&ctx, vec![1]).unwrap();
        assert_eq!(prompts(&capture).len(), 2, "current advice is not re-bought");

        run_with_ctx(&ctx, CvAdviceArgs { ids: vec![1], all: false, force: true }).unwrap();
        assert_eq!(prompts(&capture).len(), 3);

        // Editing the skill invalidates it, exactly as it does for assessments.
        let skill = dir.path().join("agent").join(AgentTask::CvAdvice.skill_path());
        let mut text = std::fs::read_to_string(&skill).unwrap();
        text.push_str("\n- Also suggest a cover-letter opening line.\n");
        std::fs::write(&skill, text).unwrap();
        advise(&ctx, vec![1]).unwrap();
        assert_eq!(prompts(&capture).len(), 4);
    }

    #[test]
    fn reassessing_drops_advice_derived_from_the_replaced_facts() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path(), &mock_agent("mock-agent.sh", None));
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();
        assess_all(&ctx).unwrap();
        advise(&ctx, vec![1]).unwrap();
        let assessment = ctx.db.latest_assessment(1).unwrap().unwrap();
        assert!(ctx.db.cv_advice_for_assessment(assessment.id).unwrap().is_some());

        assess_with_ctx(&ctx, AssessArgs { ids: vec![1], all: false, force: true }).unwrap();
        let assessment = ctx.db.latest_assessment(1).unwrap().unwrap();
        assert!(
            ctx.db.cv_advice_for_assessment(assessment.id).unwrap().is_none(),
            "advice must never outlive the facts it was written against"
        );
    }

    #[test]
    fn all_covers_assessed_jobs_and_leaves_the_rest_alone() {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("captured-prompt");
        let ctx = test_ctx(dir.path(), &mock_agent("mock-agent.sh", Some(&capture)));
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();
        assess_with_ctx(&ctx, AssessArgs { ids: vec![1], all: false, force: false }).unwrap();
        // A second job, saved but never assessed.
        ingest_payload(&ctx.db, &load_payload("sample2-same-role-other-board.json")).unwrap();

        run_with_ctx(&ctx, CvAdviceArgs { ids: vec![], all: true, force: false }).unwrap();
        let assessment = ctx.db.latest_assessment(1).unwrap().unwrap();
        assert!(ctx.db.cv_advice_for_assessment(assessment.id).unwrap().is_some());
        assert!(ctx.db.latest_assessment(2).unwrap().is_none());
        assert_eq!(prompts(&capture).len(), 2, "one assess call, one advice call");
    }
}
