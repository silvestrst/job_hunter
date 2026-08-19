pub mod assess;
pub mod config_cmd;
pub mod cv_advice;
pub mod delete;
pub mod feedback_cmd;
pub mod jobs;
pub mod rescore;
pub mod serve_cmd;
pub mod service;
pub mod show;
pub mod status_cmd;
#[cfg(test)]
pub mod test_support;

use anyhow::{Context, Result};
use job_hunter_core::config::Config;
use job_hunter_core::db::{Assessment, Db, Score};
use job_hunter_core::paths::Paths;
use job_hunter_core::scoring;
use job_hunter_core::workspace::AgentWorkspace;
use std::io::Write;
use std::sync::mpsc;
use std::time::Duration;

/// Everything a command needs: resolved paths, parsed config, open database.
pub struct Ctx {
    pub paths: Paths,
    pub config: Config,
    pub db: Db,
}

impl Ctx {
    pub fn load() -> Result<Self> {
        let paths = Paths::resolve()?;
        if !paths.config_file.exists() {
            anyhow::bail!(
                "no config at {} - run `job-hunter config init` first",
                paths.config_file.display()
            );
        }
        let config = Config::load(&paths.config_file)?;
        let db = Db::open(&paths.db_file)?;
        Ok(Self { paths, config, db })
    }

    /// The agent workspace the LLM runs in, materializing any missing
    /// instruction files from the built-in defaults (user edits are kept).
    pub fn agent_workspace(&self) -> Result<AgentWorkspace> {
        let dir = AgentWorkspace::resolve_dir(&self.config.agent, &self.paths);
        let (ws, _created) = AgentWorkspace::ensure(dir)?;
        Ok(ws)
    }
}

/// Prints a "." every `interval` while an agent call is in flight, so a single
/// job that takes the full timeout doesn't look like the CLI has hung.
pub struct Heartbeat {
    stop_tx: mpsc::Sender<()>,
    handle: std::thread::JoinHandle<()>,
}

impl Heartbeat {
    pub fn start(interval: Duration) -> Self {
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

    pub fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.handle.join();
    }
}

/// The job's newest assessment plus its score under the current criteria,
/// computing and caching the score if absent (pure Rust, no LLM).
pub fn ensure_score(
    ctx: &Ctx,
    job_id: i64,
    criteria_hash: &str,
) -> Result<Option<(Assessment, Score)>> {
    let Some(assessment) = ctx.db.latest_assessment(job_id)? else {
        return Ok(None);
    };
    if let Some(score) = ctx.db.get_score(assessment.id, criteria_hash)? {
        return Ok(Some((assessment, score)));
    }
    let facts = assessment.facts()?;
    let result = scoring::compute_score(&ctx.config.criteria, &facts)
        .with_context(|| format!("scoring job {job_id}"))?;
    let breakdown_json = serde_json::to_string(&result)?;
    ctx.db.upsert_score(
        job_id,
        assessment.id,
        criteria_hash,
        result.score,
        result.hard_filter_passed,
        &breakdown_json,
    )?;
    let score = ctx
        .db
        .get_score(assessment.id, criteria_hash)?
        .expect("score row just inserted");
    Ok(Some((assessment, score)))
}

/// "£85k-100k" style compact salary from stored facts, for table cells.
pub fn format_salary(facts: &job_hunter_core::assessment::schema::AssessmentFacts) -> String {
    let salary = &facts.extracted.salary;
    let unit = |v: f64| -> String {
        if v >= 1000.0 {
            format!("{:.0}k", v / 1000.0)
        } else {
            format!("{v:.0}")
        }
    };
    let currency = salary.currency.as_deref().unwrap_or("");
    let symbol = match currency {
        "GBP" => "£",
        "USD" => "$",
        "EUR" => "€",
        other => other,
    };
    let period = match salary.period {
        job_hunter_core::assessment::schema::SalaryPeriod::Day => "/day",
        job_hunter_core::assessment::schema::SalaryPeriod::Hour => "/hr",
        _ => "",
    };
    let text = match (salary.min, salary.max) {
        (Some(min), Some(max)) if min != max => {
            format!("{symbol}{}-{}{period}", unit(min), unit(max))
        }
        (Some(v), _) | (None, Some(v)) => format!("{symbol}{}{period}", unit(v)),
        (None, None) => return "-".to_string(),
    };
    if salary.is_estimate {
        format!("{text}~")
    } else {
        text
    }
}

/// Days since an RFC3339 timestamp, e.g. "3d".
pub fn format_age(created_at: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(created_at) {
        Ok(t) => {
            let days = (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_days();
            format!("{days}d")
        }
        Err(_) => "?".to_string(),
    }
}

pub fn job_label(job: &job_hunter_core::db::Job) -> String {
    match (&job.title, &job.company) {
        (Some(t), Some(c)) => format!("{t} @ {c}"),
        (Some(t), None) => t.clone(),
        _ => format!("job #{}", job.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use job_hunter_core::assessment::schema::AssessmentFacts;

    #[test]
    fn estimated_salary_is_marked() {
        let mut facts = AssessmentFacts::default();
        facts.extracted.salary.min = Some(90_000.0);
        facts.extracted.salary.max = Some(110_000.0);
        facts.extracted.salary.currency = Some("GBP".into());
        assert_eq!(format_salary(&facts), "£90k-110k");

        facts.extracted.salary.is_estimate = true;
        assert_eq!(format_salary(&facts), "£90k-110k~");
    }

    #[test]
    fn missing_salary_has_no_estimate_marker() {
        let facts = AssessmentFacts::default();
        assert_eq!(format_salary(&facts), "-");
    }
}
