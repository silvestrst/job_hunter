use super::{ensure_score, job_label, Ctx};
use anyhow::{bail, Context, Result};
use clap::Args;
use job_hunter_core::agent::AgentRunner;
use job_hunter_core::assessment::prompt;
use job_hunter_core::assessment::schema::CURRENT_SCHEMA_VERSION;
use job_hunter_core::db::{JobFilter, JobStatus};
use job_hunter_core::{feedback, normalize_identity, paths::expand_tilde, sha256_hex};
use std::io::Write;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Prints a "." every `interval` while an agent call is in flight, so a single
/// job that takes the full timeout doesn't look like the CLI has hung.
struct Heartbeat {
    stop_tx: mpsc::Sender<()>,
    handle: std::thread::JoinHandle<()>,
}

impl Heartbeat {
    fn start(interval: Duration) -> Self {
        let (stop_tx, stop_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || loop {
            match stop_rx.recv_timeout(interval) {
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    print!(".");
                    let _ = std::io::stdout().flush();
                }
                _ => break,
            }
        });
        Self { stop_tx, handle }
    }

    fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.handle.join();
    }
}

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

pub fn run(args: AssessArgs) -> Result<()> {
    run_with_ctx(&Ctx::load()?, args)
}

pub fn run_with_ctx(ctx: &Ctx, args: AssessArgs) -> Result<()> {
    let criteria_hash = ctx.config.criteria_hash();
    let prompt_version = ctx.config.assessment.prompt_version;

    let cv_path = expand_tilde(&ctx.config.resources.cv_path);
    let cv_text = std::fs::read_to_string(&cv_path)
        .with_context(|| format!("cannot read CV at {} (resources.cv_path)", cv_path.display()))?;
    let cv_hash = sha256_hex(cv_text.as_bytes());

    // Which jobs need the LLM?
    let mut targets = Vec::new();
    if args.ids.is_empty() {
        let candidates = ctx.db.list_jobs(&JobFilter {
            statuses: Some(vec![JobStatus::New, JobStatus::Assessed]),
            include_duplicates: false,
        })?;
        for job in candidates {
            if args.force || !ctx.db.assessment_exists(job.id, &cv_hash, prompt_version)? {
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
            if !args.force && ctx.db.assessment_exists(job.id, &cv_hash, prompt_version)? {
                println!("job #{id} already assessed under the current CV/prompt - use --force to re-run");
                continue;
            }
            targets.push(job);
        }
    }
    if targets.is_empty() {
        println!("Nothing to assess.");
        return Ok(());
    }

    let runner = AgentRunner::new(&ctx.config.agent);
    let cv_prompt_text = cv_for_prompt(&ctx, &runner, &cv_text, &cv_hash)?;
    let feedback_digest = feedback::build_prompt_digest(&ctx.db, ctx.config.assessment.feedback_recent_n)?;
    let feedback_digest_hash = feedback_digest.as_deref().map(|d| sha256_hex(d.as_bytes())[..16].to_string());
    let agent_command = ctx.config.agent.command.join(" ");

    let total = targets.len();
    println!("Assessing {total} job(s) with `{agent_command}`...\n");
    let overall_start = Instant::now();
    let mut failures = 0usize;
    for (i, job) in targets.iter().enumerate() {
        let assessment_prompt = prompt::build_assessment_prompt(
            &cv_prompt_text,
            feedback_digest.as_deref(),
            &job.content_text,
            ctx.config.assessment.max_job_chars,
        );
        print!("[{}/{total}] job #{} ({})... ", i + 1, job.id, job_label(job));
        std::io::stdout().flush().ok();

        let heartbeat = Heartbeat::start(Duration::from_secs(5));
        let started = Instant::now();
        let result = runner.run_facts(&assessment_prompt);
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

        ctx.db.upsert_assessment(
            job.id,
            &cv_hash,
            prompt_version,
            if facts.schema_version > 0 { facts.schema_version } else { CURRENT_SCHEMA_VERSION },
            feedback_digest_hash.as_deref(),
            &agent_command,
            &facts_json,
        )?;

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

        let scored = ensure_score(&ctx, job.id, &criteria_hash)?;
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
    use job_hunter_core::config::Config;
    use job_hunter_core::db::Db;
    use job_hunter_core::ingest::{ingest_payload, ScrapedPayload};
    use job_hunter_core::paths::Paths;
    use std::path::{Path, PathBuf};

    fn fixtures_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").canonicalize().unwrap()
    }

    /// Ctx over a temp dir, configured with the given agent argv.
    fn test_ctx(dir: &Path, agent_command: &[String]) -> Ctx {
        let fixtures = fixtures_dir();
        let command_yaml: Vec<String> =
            agent_command.iter().map(|c| format!("{c:?}")).collect();
        let config_yaml = format!(
            r#"
resources:
  cv_path: {cv}
agent:
  command: [{command}]
  output: claude_json
  timeout_secs: 30
  retries: 2
criteria:
  currency_rates_to_gbp: {{USD: 0.78, EUR: 0.86}}
  hard_filters:
    - {{fact: salary_max_gbp, op: gte, value: 85000, if_missing: pass}}
    - {{fact: work_mode, op: in, value: [remote, hybrid], if_missing: fail}}
  soft:
    - {{fact: overall_cv_match, weight: 5}}
    - {{fact: seniority_fit, weight: 3}}
    - {{fact: salary_max_gbp, weight: 3, target: 120000}}
    - {{fact: domain_familiarity, weight: 1}}
"#,
            cv = fixtures.join("sample-cv.md").display(),
            command = command_yaml.join(", "),
        );
        let config_file = dir.join("config.yaml");
        std::fs::write(&config_file, config_yaml).unwrap();
        let config = Config::load(&config_file).unwrap();
        let db = Db::open(&dir.join("job_hunter.db")).unwrap();
        Ctx {
            paths: Paths {
                config_file,
                data_dir: dir.to_path_buf(),
                db_file: dir.join("job_hunter.db"),
                api_token_file: dir.join("api_token"),
                cv_digest_dir: dir.join("cv_digest"),
                systemd_unit_file: dir.join("job-hunter.service"),
            },
            config,
            db,
        }
    }

    fn load_payload(name: &str) -> ScrapedPayload {
        let text = std::fs::read_to_string(fixtures_dir().join("payloads").join(name)).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn assess_all(ctx: &Ctx) -> Result<()> {
        run_with_ctx(ctx, AssessArgs { ids: vec![], all: true, force: false })
    }

    #[test]
    fn full_pipeline_assesses_scores_and_links_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let mock = fixtures_dir().join("mock-agent.sh");
        let ctx = test_ctx(dir.path(), &[mock.to_string_lossy().into_owned()]);

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
        let mock = fixtures_dir().join("mock-agent.sh");
        let mut ctx = test_ctx(dir.path(), &[mock.to_string_lossy().into_owned()]);
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();
        assess_all(&ctx).unwrap();
        let hash_before = ctx.config.criteria_hash();
        let (_, before) = ensure_score(&ctx, 1, &hash_before).unwrap().unwrap();

        // Tighten the salary filter; swap the agent for one that always fails
        // to prove rescoring never invokes it.
        ctx.config.criteria.hard_filters[0].value = serde_json::json!(150000);
        ctx.config.agent.command = vec![fixtures_dir().join("mock-agent-fail.sh").to_string_lossy().into_owned()];
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
        let script = fixtures_dir().join("mock-agent-fail.sh");
        let ctx = test_ctx(dir.path(), &[script.to_string_lossy().into_owned()]);
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();
        let err = assess_all(&ctx).unwrap_err();
        assert!(format!("{err}").contains("1 of 1"));
        assert!(ctx.db.latest_assessment(1).unwrap().is_none());
        assert_eq!(ctx.db.get_job(1).unwrap().status, "new");
    }

    #[test]
    fn feedback_digest_is_injected_into_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("captured-prompt");
        let script = fixtures_dir().join("mock-agent.sh");
        let command = vec![
            "env".to_string(),
            format!("MOCK_CAPTURE_FILE={}", capture.display()),
            script.to_string_lossy().into_owned(),
        ];
        let ctx = test_ctx(dir.path(), &command);
        ingest_payload(&ctx.db, &load_payload("sample1.json")).unwrap();
        ctx.db.insert_feedback(None, "never rank agencies above product companies").unwrap();

        assess_all(&ctx).unwrap();
        let prompt = std::fs::read_to_string(&capture).unwrap();
        assert!(prompt.contains("never rank agencies above product companies"));
        assert!(prompt.contains("Test Candidate"), "CV embedded verbatim");
        assert!(prompt.contains("Senior Rust Engineer"), "job text embedded");
    }
}
