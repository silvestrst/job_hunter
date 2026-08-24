pub mod migrations;

use crate::now_utc;
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    New,
    Assessed,
    Applied,
    Progressing,
    Rejected,
    Dismissed,
}

impl JobStatus {
    pub const ALL: &'static [JobStatus] = &[
        JobStatus::New,
        JobStatus::Assessed,
        JobStatus::Applied,
        JobStatus::Progressing,
        JobStatus::Rejected,
        JobStatus::Dismissed,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::New => "new",
            JobStatus::Assessed => "assessed",
            JobStatus::Applied => "applied",
            JobStatus::Progressing => "progressing",
            JobStatus::Rejected => "rejected",
            JobStatus::Dismissed => "dismissed",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|v| v.as_str() == s)
            .with_context(|| {
                let valid: Vec<_> = Self::ALL.iter().map(|v| v.as_str()).collect();
                format!("invalid status '{s}' (valid: {})", valid.join(", "))
            })
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: i64,
    pub url: String,
    pub url_normalized: String,
    pub domain: String,
    pub title: Option<String>,
    pub company: Option<String>,
    pub location: Option<String>,
    pub duplicate_of: Option<i64>,
    pub content_text: String,
    pub source: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct Assessment {
    pub id: i64,
    pub job_id: i64,
    pub experience_hash: String,
    pub prompt_version: i64,
    pub schema_version: i64,
    pub instructions_hash: Option<String>,
    /// None when no preferences file was configured.
    pub preferences_hash: Option<String>,
    pub facts_json: String,
    pub created_at: String,
}

impl Assessment {
    pub fn facts(&self) -> Result<crate::assessment::schema::AssessmentFacts> {
        serde_json::from_str(&self.facts_json).context("stored facts_json no longer parses")
    }
}

/// Everything an assessment row records. A struct rather than nine
/// positional arguments because four of them are interchangeable strings.
#[derive(Debug, Clone)]
pub struct NewAssessment<'a> {
    pub job_id: i64,
    pub experience_hash: &'a str,
    pub preferences_hash: Option<&'a str>,
    pub prompt_version: i64,
    pub schema_version: i64,
    pub feedback_digest_hash: Option<&'a str>,
    pub instructions_hash: Option<&'a str>,
    pub agent_command: &'a str,
    pub facts_json: &'a str,
}

/// What makes a stored assessment current: everything it was derived from
/// besides the posting itself.
#[derive(Debug, Clone)]
pub struct AssessmentKey<'a> {
    pub experience_hash: &'a str,
    pub preferences_hash: Option<&'a str>,
    pub prompt_version: i64,
    pub instructions_hash: &'a str,
}

#[derive(Debug, Clone)]
pub struct Score {
    pub assessment_id: i64,
    pub criteria_hash: String,
    pub score: f64,
    pub hard_filter_passed: bool,
    pub breakdown_json: String,
}

impl Score {
    /// The score's components and filter results. Fields added since a row was
    /// written default rather than failing the parse, so an older cached
    /// breakdown still renders.
    pub fn breakdown(&self) -> Result<crate::scoring::ScoreResult> {
        serde_json::from_str(&self.breakdown_json)
            .context("stored breakdown_json no longer parses")
    }
}

#[derive(Debug, Clone)]
pub struct FeedbackItem {
    pub id: i64,
    pub job_id: Option<i64>,
    pub text: String,
    pub state: String,
    pub created_at: String,
    /// "Title @ Company" when the feedback is linked to a job.
    pub job_label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StatusEvent {
    pub from_status: Option<String>,
    pub to_status: String,
    pub note: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct JobFilter {
    /// None = any status.
    pub statuses: Option<Vec<JobStatus>>,
    pub include_duplicates: bool,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("cannot open database {}", path.display()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrations::migrations()
            .to_latest(&mut conn)
            .context("database migration failed")?;
        Ok(Self { conn })
    }

    // ---- jobs -------------------------------------------------------------

    /// Insert a scraped job; returns (id, was_duplicate) keyed on the
    /// normalized URL.
    pub fn insert_job(
        &self,
        url: &str,
        url_normalized: &str,
        domain: &str,
        title: Option<&str>,
        content_text: &str,
        raw_html: Option<&str>,
        source: &str,
    ) -> Result<(i64, bool)> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM jobs WHERE url_normalized = ?1",
                [url_normalized],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            return Ok((id, true));
        }
        let now = now_utc();
        self.conn.execute(
            "INSERT INTO jobs (url, url_normalized, domain, title, content_text, raw_html, source, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![url, url_normalized, domain, title, content_text, raw_html, source, now],
        )?;
        Ok((self.conn.last_insert_rowid(), false))
    }

    fn job_from_row(row: &Row) -> rusqlite::Result<Job> {
        Ok(Job {
            id: row.get("id")?,
            url: row.get("url")?,
            url_normalized: row.get("url_normalized")?,
            domain: row.get("domain")?,
            title: row.get("title")?,
            company: row.get("company")?,
            location: row.get("location")?,
            duplicate_of: row.get("duplicate_of")?,
            content_text: row.get("content_text")?,
            source: row.get("source")?,
            status: row.get("status")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }

    const JOB_COLUMNS: &'static str = "id, url, url_normalized, domain, title, company, location, \
                                       duplicate_of, content_text, source, status, created_at, updated_at";

    pub fn get_job(&self, id: i64) -> Result<Job> {
        self.conn
            .query_row(
                &format!("SELECT {} FROM jobs WHERE id = ?1", Self::JOB_COLUMNS),
                [id],
                Self::job_from_row,
            )
            .optional()?
            .with_context(|| format!("no job with id {id}"))
    }

    pub fn list_jobs(&self, filter: &JobFilter) -> Result<Vec<Job>> {
        let mut sql = format!("SELECT {} FROM jobs WHERE 1=1", Self::JOB_COLUMNS);
        if !filter.include_duplicates {
            sql.push_str(" AND duplicate_of IS NULL");
        }
        if let Some(statuses) = &filter.statuses {
            let list: Vec<String> = statuses.iter().map(|s| format!("'{}'", s.as_str())).collect();
            sql.push_str(&format!(" AND status IN ({})", list.join(",")));
        }
        sql.push_str(" ORDER BY id");
        let mut stmt = self.conn.prepare(&sql)?;
        let jobs = stmt
            .query_map([], Self::job_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(jobs)
    }

    pub fn set_status(&self, id: i64, to: JobStatus, note: Option<&str>) -> Result<Job> {
        let job = self.get_job(id)?;
        if job.status == to.as_str() {
            bail!("job {id} is already '{}'", to.as_str());
        }
        let now = now_utc();
        self.conn.execute(
            "UPDATE jobs SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![to.as_str(), now, id],
        )?;
        self.conn.execute(
            "INSERT INTO status_events (job_id, from_status, to_status, note, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, job.status, to.as_str(), note, now],
        )?;
        self.get_job(id)
    }

    pub fn delete_job(&self, id: i64) -> Result<()> {
        let n = self.conn.execute("DELETE FROM jobs WHERE id = ?1", [id])?;
        if n == 0 {
            bail!("no job with id {id}");
        }
        Ok(())
    }

    /// Another non-duplicate job with the same normalized title+company, if any.
    pub fn find_title_company_match(
        &self,
        title_normalized: &str,
        company_normalized: &str,
        exclude_id: i64,
    ) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM jobs
                 WHERE title_normalized = ?1 AND company_normalized = ?2
                   AND duplicate_of IS NULL AND id != ?3
                 ORDER BY id LIMIT 1",
                params![title_normalized, company_normalized, exclude_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Write LLM-extracted identity fields after assessment; `duplicate_of`
    /// is set in the same UPDATE so the partial unique index never trips.
    #[allow(clippy::too_many_arguments)]
    pub fn backfill_extraction(
        &self,
        id: i64,
        title: Option<&str>,
        company: Option<&str>,
        location: Option<&str>,
        title_normalized: Option<&str>,
        company_normalized: Option<&str>,
        duplicate_of: Option<i64>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE jobs SET title = COALESCE(?1, title), company = COALESCE(?2, company),
                             location = COALESCE(?3, location),
                             title_normalized = ?4, company_normalized = ?5,
                             duplicate_of = ?6, updated_at = ?7
             WHERE id = ?8",
            params![title, company, location, title_normalized, company_normalized, duplicate_of, now_utc(), id],
        )?;
        Ok(())
    }

    pub fn duplicates_of(&self, id: i64) -> Result<Vec<Job>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM jobs WHERE duplicate_of = ?1 ORDER BY id",
            Self::JOB_COLUMNS
        ))?;
        let jobs = stmt
            .query_map([id], Self::job_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(jobs)
    }

    pub fn status_events(&self, job_id: i64) -> Result<Vec<StatusEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT from_status, to_status, note, created_at FROM status_events
             WHERE job_id = ?1 ORDER BY id",
        )?;
        let events = stmt
            .query_map([job_id], |r| {
                Ok(StatusEvent {
                    from_status: r.get(0)?,
                    to_status: r.get(1)?,
                    note: r.get(2)?,
                    created_at: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(events)
    }

    // ---- assessments ------------------------------------------------------

    /// Insert or replace the assessment for
    /// (job, experience_hash, prompt_version). Replacing the facts drops the
    /// scores cached from them, rather than leaving stale conclusions
    /// attached to new facts.
    pub fn upsert_assessment(&self, a: &NewAssessment) -> Result<i64> {
        let id: i64 = self.conn.query_row(
            "INSERT INTO assessments (job_id, experience_hash, preferences_hash, prompt_version,
                                      schema_version, feedback_digest_hash, instructions_hash,
                                      agent_command, facts_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(job_id, experience_hash, prompt_version) DO UPDATE SET
                preferences_hash = excluded.preferences_hash,
                schema_version = excluded.schema_version,
                feedback_digest_hash = excluded.feedback_digest_hash,
                instructions_hash = excluded.instructions_hash,
                agent_command = excluded.agent_command,
                facts_json = excluded.facts_json,
                created_at = excluded.created_at
             RETURNING id",
            params![a.job_id, a.experience_hash, a.preferences_hash, a.prompt_version,
                    a.schema_version, a.feedback_digest_hash, a.instructions_hash, a.agent_command,
                    a.facts_json, now_utc()],
            |r| r.get(0),
        )?;
        self.conn
            .execute("DELETE FROM scores WHERE assessment_id = ?1", [id])?;
        Ok(id)
    }

    /// Newest assessment for a job regardless of version (jobs assessed under
    /// an older experience file keep showing that score until re-assessed).
    pub fn latest_assessment(&self, job_id: i64) -> Result<Option<Assessment>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, job_id, experience_hash, prompt_version, schema_version,
                        instructions_hash, preferences_hash, facts_json, created_at
                 FROM assessments WHERE job_id = ?1 ORDER BY id DESC LIMIT 1",
                [job_id],
                |r| {
                    Ok(Assessment {
                        id: r.get(0)?,
                        job_id: r.get(1)?,
                        experience_hash: r.get(2)?,
                        prompt_version: r.get(3)?,
                        schema_version: r.get(4)?,
                        instructions_hash: r.get(5)?,
                        preferences_hash: r.get(6)?,
                        facts_json: r.get(7)?,
                        created_at: r.get(8)?,
                    })
                },
            )
            .optional()?)
    }

    /// Whether a current assessment exists. Rows with a NULL
    /// instructions_hash predate the agent workspace and are grandfathered as
    /// matching, so upgrading never silently re-spends tokens.
    /// preferences_hash needs no such carve-out: it is compared NULL-safely
    /// with `IS`, so a row written with no preferences file keeps matching a
    /// profile that still has none - and stops matching the moment one is
    /// added, which is a deliberate act like editing the experience file, not
    /// an upgrade.
    pub fn assessment_exists(&self, job_id: i64, key: &AssessmentKey) -> Result<bool> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM assessments
                 WHERE job_id = ?1 AND experience_hash = ?2 AND prompt_version = ?3
                   AND (instructions_hash IS NULL OR instructions_hash = ?4)
                   AND preferences_hash IS ?5",
                params![job_id, key.experience_hash, key.prompt_version, key.instructions_hash,
                        key.preferences_hash],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    // ---- scores -----------------------------------------------------------

    pub fn get_score(&self, assessment_id: i64, criteria_hash: &str) -> Result<Option<Score>> {
        Ok(self
            .conn
            .query_row(
                "SELECT assessment_id, criteria_hash, score, hard_filter_passed, breakdown_json
                 FROM scores WHERE assessment_id = ?1 AND criteria_hash = ?2",
                params![assessment_id, criteria_hash],
                |r| {
                    Ok(Score {
                        assessment_id: r.get(0)?,
                        criteria_hash: r.get(1)?,
                        score: r.get(2)?,
                        hard_filter_passed: r.get::<_, i64>(3)? != 0,
                        breakdown_json: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    /// Most recent score computed for this assessment under a *different*
    /// criteria hash — used by `rescore` to show deltas after criteria edits.
    pub fn previous_score(&self, assessment_id: i64, exclude_hash: &str) -> Result<Option<Score>> {
        Ok(self
            .conn
            .query_row(
                "SELECT assessment_id, criteria_hash, score, hard_filter_passed, breakdown_json
                 FROM scores WHERE assessment_id = ?1 AND criteria_hash != ?2
                 ORDER BY created_at DESC, id DESC LIMIT 1",
                params![assessment_id, exclude_hash],
                |r| {
                    Ok(Score {
                        assessment_id: r.get(0)?,
                        criteria_hash: r.get(1)?,
                        score: r.get(2)?,
                        hard_filter_passed: r.get::<_, i64>(3)? != 0,
                        breakdown_json: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn upsert_score(
        &self,
        job_id: i64,
        assessment_id: i64,
        criteria_hash: &str,
        score: f64,
        hard_filter_passed: bool,
        breakdown_json: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO scores (job_id, assessment_id, criteria_hash, score, hard_filter_passed, breakdown_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(assessment_id, criteria_hash) DO UPDATE SET
                score = excluded.score,
                hard_filter_passed = excluded.hard_filter_passed,
                breakdown_json = excluded.breakdown_json,
                created_at = excluded.created_at",
            params![job_id, assessment_id, criteria_hash, score, hard_filter_passed as i64, breakdown_json, now_utc()],
        )?;
        Ok(())
    }

    // ---- feedback ---------------------------------------------------------

    pub fn insert_feedback(&self, job_id: Option<i64>, text: &str) -> Result<i64> {
        if let Some(id) = job_id {
            self.get_job(id)?; // fail early on a bad id
        }
        self.conn.execute(
            "INSERT INTO feedback (job_id, text, created_at) VALUES (?1, ?2, ?3)",
            params![job_id, text, now_utc()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn active_feedback(&self, limit: Option<usize>) -> Result<Vec<FeedbackItem>> {
        let sql = format!(
            "SELECT f.id, f.job_id, f.text, f.state, f.created_at,
                    CASE WHEN j.id IS NULL THEN NULL
                         ELSE COALESCE(j.title, 'job #' || j.id) || COALESCE(' @ ' || j.company, '')
                    END
             FROM feedback f LEFT JOIN jobs j ON j.id = f.job_id
             WHERE f.state = 'active' ORDER BY f.id DESC{}",
            limit.map(|n| format!(" LIMIT {n}")).unwrap_or_default()
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let items = stmt
            .query_map([], |r| {
                Ok(FeedbackItem {
                    id: r.get(0)?,
                    job_id: r.get(1)?,
                    text: r.get(2)?,
                    state: r.get(3)?,
                    created_at: r.get(4)?,
                    job_label: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(items)
    }

    pub fn feedback_for_job(&self, job_id: i64) -> Result<Vec<FeedbackItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, job_id, text, state, created_at, NULL FROM feedback
             WHERE job_id = ?1 ORDER BY id",
        )?;
        let items = stmt
            .query_map([job_id], |r| {
                Ok(FeedbackItem {
                    id: r.get(0)?,
                    job_id: r.get(1)?,
                    text: r.get(2)?,
                    state: r.get(3)?,
                    created_at: r.get(4)?,
                    job_label: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(items)
    }

    pub fn mark_all_feedback_compacted(&self) -> Result<usize> {
        Ok(self
            .conn
            .execute("UPDATE feedback SET state = 'compacted' WHERE state = 'active'", [])?)
    }

    pub fn get_feedback_digest(&self) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT digest FROM feedback_digest WHERE id = 1", [], |r| r.get(0))
            .optional()?)
    }

    pub fn set_feedback_digest(&self, digest: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO feedback_digest (id, digest, updated_at) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET digest = excluded.digest, updated_at = excluded.updated_at",
            params![digest, now_utc()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded_db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.insert_job(
            "https://example.com/jobs/1?utm_source=x",
            "https://example.com/jobs/1",
            "example.com",
            Some("Job One"),
            "text one",
            None,
            "extension",
        )
        .unwrap();
        db
    }

    #[test]
    fn insert_dedupes_on_normalized_url() {
        let db = seeded_db();
        let (id, dup) = db
            .insert_job(
                "https://example.com/jobs/1",
                "https://example.com/jobs/1",
                "example.com",
                None,
                "text",
                None,
                "extension",
            )
            .unwrap();
        assert_eq!(id, 1);
        assert!(dup);
    }

    #[test]
    fn status_transition_records_event() {
        let db = seeded_db();
        let job = db.set_status(1, JobStatus::Applied, Some("sent CV")).unwrap();
        assert_eq!(job.status, "applied");
        let events = db.status_events(1).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].from_status.as_deref(), Some("new"));
        assert_eq!(events[0].note.as_deref(), Some("sent CV"));
    }

    fn assessment<'a>(
        experience_hash: &'a str,
        instructions: Option<&'a str>,
        facts: &'a str,
    ) -> NewAssessment<'a> {
        NewAssessment {
            job_id: 1,
            experience_hash,
            preferences_hash: None,
            prompt_version: 1,
            schema_version: 1,
            feedback_digest_hash: None,
            instructions_hash: instructions,
            agent_command: "mock",
            facts_json: facts,
        }
    }

    fn key<'a>(experience_hash: &'a str, instructions: &'a str) -> AssessmentKey<'a> {
        AssessmentKey {
            experience_hash,
            preferences_hash: None,
            prompt_version: 1,
            instructions_hash: instructions,
        }
    }

    #[test]
    fn assessment_upsert_replaces_and_drops_derived_rows() {
        let db = seeded_db();
        let a1 = db
            .upsert_assessment(&assessment("exp1", Some("aaaa"), r#"{"schema_version":1}"#))
            .unwrap();
        db.upsert_score(1, a1, "crit1", 80.0, true, "[]").unwrap();
        assert!(db.get_score(a1, "crit1").unwrap().is_some());

        let a2 = db
            .upsert_assessment(&assessment("exp1", Some("bbbb"), r#"{"schema_version":2}"#))
            .unwrap();
        assert_eq!(a1, a2);
        assert!(
            db.get_score(a2, "crit1").unwrap().is_none(),
            "scores derived from replaced facts must not survive them"
        );

        let latest = db.latest_assessment(1).unwrap().unwrap();
        assert!(latest.facts_json.contains("\"schema_version\":2"));
        assert_eq!(latest.instructions_hash.as_deref(), Some("bbbb"));
    }

    #[test]
    fn assessment_exists_keys_on_instructions_hash() {
        let db = seeded_db();
        db.upsert_assessment(&assessment("exp1", Some("aaaa"), "{}")).unwrap();
        assert!(db.assessment_exists(1, &key("exp1", "aaaa")).unwrap());
        assert!(!db.assessment_exists(1, &key("exp1", "bbbb")).unwrap(), "instruction edits re-run");

        // NULL hash = pre-workspace row, grandfathered under any hash.
        db.upsert_assessment(&assessment("exp2", None, "{}")).unwrap();
        assert!(db.assessment_exists(1, &key("exp2", "anything")).unwrap());
    }

    #[test]
    fn assessment_exists_keys_on_preferences_hash() {
        let db = seeded_db();
        // Written with no preferences file: still current while there is none.
        db.upsert_assessment(&assessment("exp1", Some("aaaa"), "{}")).unwrap();
        assert!(db.assessment_exists(1, &key("exp1", "aaaa")).unwrap());

        // Adding preferences is a deliberate act, like editing the experience.
        let with_prefs = AssessmentKey { preferences_hash: Some("p1"), ..key("exp1", "aaaa") };
        assert!(!db.assessment_exists(1, &with_prefs).unwrap());

        db.upsert_assessment(&NewAssessment {
            preferences_hash: Some("p1"),
            ..assessment("exp1", Some("aaaa"), "{}")
        })
        .unwrap();
        assert!(db.assessment_exists(1, &with_prefs).unwrap());
        assert!(
            !db.assessment_exists(1, &AssessmentKey { preferences_hash: Some("p2"), ..with_prefs.clone() }).unwrap(),
            "editing preferences re-runs"
        );
        assert!(!db.assessment_exists(1, &key("exp1", "aaaa")).unwrap(), "removing them re-runs too");
    }

    #[test]
    fn duplicate_linking_via_backfill() {
        let db = seeded_db();
        db.insert_job(
            "https://other.com/jobs/9",
            "https://other.com/jobs/9",
            "other.com",
            None,
            "text",
            None,
            "extension",
        )
        .unwrap();
        db.backfill_extraction(1, Some("Rust Dev"), Some("Acme"), None, Some("rust dev"), Some("acme"), None)
            .unwrap();
        let dup = db.find_title_company_match("rust dev", "acme", 2).unwrap();
        assert_eq!(dup, Some(1));
        db.backfill_extraction(2, Some("Rust Dev"), Some("Acme"), None, Some("rust dev"), Some("acme"), dup)
            .unwrap();
        assert_eq!(db.get_job(2).unwrap().duplicate_of, Some(1));
        assert_eq!(db.duplicates_of(1).unwrap().len(), 1);
        // Default listing hides the duplicate.
        assert_eq!(db.list_jobs(&JobFilter::default()).unwrap().len(), 1);
    }

    #[test]
    fn feedback_digest_roundtrip() {
        let db = seeded_db();
        db.insert_feedback(Some(1), "scored too high").unwrap();
        db.insert_feedback(None, "prefer smaller companies").unwrap();
        let items = db.active_feedback(None).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].job_label.as_deref(), Some("Job One"));

        assert!(db.get_feedback_digest().unwrap().is_none());
        db.set_feedback_digest("rule 1").unwrap();
        db.set_feedback_digest("rule 2").unwrap();
        assert_eq!(db.get_feedback_digest().unwrap().unwrap(), "rule 2");

        assert_eq!(db.mark_all_feedback_compacted().unwrap(), 2);
        assert!(db.active_feedback(None).unwrap().is_empty());
    }
}
