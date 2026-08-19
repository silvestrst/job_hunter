use rusqlite_migration::{Migrations, M};

pub fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(
        r#"
CREATE TABLE jobs (
  id             INTEGER PRIMARY KEY,
  url            TEXT NOT NULL,
  url_normalized TEXT NOT NULL UNIQUE,
  domain         TEXT NOT NULL,
  title          TEXT,
  company        TEXT,
  location       TEXT,
  title_normalized   TEXT,
  company_normalized TEXT,
  duplicate_of   INTEGER REFERENCES jobs(id),
  content_text   TEXT NOT NULL,
  raw_html       TEXT,
  source         TEXT NOT NULL DEFAULT 'extension',
  status         TEXT NOT NULL DEFAULT 'new',
  created_at     TEXT NOT NULL,
  updated_at     TEXT NOT NULL
);

CREATE TABLE assessments (
  id             INTEGER PRIMARY KEY,
  job_id         INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
  cv_hash        TEXT NOT NULL,
  prompt_version INTEGER NOT NULL,
  schema_version INTEGER NOT NULL,
  feedback_digest_hash TEXT,
  agent_command  TEXT,
  facts_json     TEXT NOT NULL,
  created_at     TEXT NOT NULL,
  UNIQUE(job_id, cv_hash, prompt_version)
);

CREATE TABLE scores (
  id             INTEGER PRIMARY KEY,
  job_id         INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
  assessment_id  INTEGER NOT NULL REFERENCES assessments(id) ON DELETE CASCADE,
  criteria_hash  TEXT NOT NULL,
  score          REAL NOT NULL,
  hard_filter_passed INTEGER NOT NULL,
  breakdown_json TEXT NOT NULL,
  created_at     TEXT NOT NULL,
  UNIQUE(assessment_id, criteria_hash)
);

CREATE TABLE feedback (
  id         INTEGER PRIMARY KEY,
  job_id     INTEGER REFERENCES jobs(id) ON DELETE SET NULL,
  text       TEXT NOT NULL,
  state      TEXT NOT NULL DEFAULT 'active',
  created_at TEXT NOT NULL
);

CREATE TABLE feedback_digest (
  id         INTEGER PRIMARY KEY CHECK (id = 1),
  digest     TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE status_events (
  id          INTEGER PRIMARY KEY,
  job_id      INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
  from_status TEXT,
  to_status   TEXT NOT NULL,
  note        TEXT,
  created_at  TEXT NOT NULL
);

CREATE INDEX idx_jobs_status ON jobs(status);
CREATE INDEX idx_assessments_job ON assessments(job_id, cv_hash, prompt_version);
CREATE UNIQUE INDEX idx_jobs_title_company ON jobs(title_normalized, company_normalized)
  WHERE title_normalized IS NOT NULL AND company_normalized IS NOT NULL AND duplicate_of IS NULL;
"#,
    ),
    // Hash of the agent workspace instructions (AGENTS.md + task skill) in
    // effect when the assessment ran. NULL on pre-workspace rows, which are
    // grandfathered - see Db::assessment_exists.
    M::up("ALTER TABLE assessments ADD COLUMN instructions_hash TEXT;"),
    // Hash of the supplementary profile documents (background, preferences)
    // in effect for the assessment; NULL means "none configured", compared
    // NULL-safely with IS - see Db::assessment_exists. Plus the CV advice
    // `cv-advice` produces, one row per assessment: derived from that
    // assessment's facts, so it is dropped whenever those facts are replaced.
    M::up(
        r#"
ALTER TABLE assessments ADD COLUMN profile_hash TEXT;

CREATE TABLE cv_advice (
  id                INTEGER PRIMARY KEY,
  job_id            INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
  assessment_id     INTEGER NOT NULL REFERENCES assessments(id) ON DELETE CASCADE,
  schema_version    INTEGER NOT NULL,
  profile_hash      TEXT,
  instructions_hash TEXT,
  agent_command     TEXT,
  advice_json       TEXT NOT NULL,
  created_at        TEXT NOT NULL,
  UNIQUE(assessment_id)
);

CREATE INDEX idx_cv_advice_job ON cv_advice(job_id);
"#,
    )])
}

#[cfg(test)]
mod tests {
    #[test]
    fn migrations_are_valid() {
        super::migrations().validate().unwrap();
    }
}
