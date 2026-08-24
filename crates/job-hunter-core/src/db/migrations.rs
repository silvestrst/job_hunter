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
    // `cv-advice` produced, one row per assessment. Both the `cv-advice`
    // command and the CV/background split are gone; migration 4 below cleans
    // up after them. This one is kept verbatim because migrations are
    // append-only history, not a description of the current schema.
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
    ),
    // The CV/background split collapsed into one experience document, and
    // `cv-advice` was dropped. Renaming rather than recreating keeps existing
    // rows: SQLite rewrites the column references in the UNIQUE constraint and
    // in idx_assessments_job for us. Rows written before the merge keep the
    // hash of the old CV file, which no longer matches the merged document -
    // so they fall out of date and re-assess exactly like any other edit to
    // the evidence, which is the correct outcome rather than an accident.
    M::up(
        r#"
DROP TABLE cv_advice;

ALTER TABLE assessments RENAME COLUMN cv_hash TO experience_hash;
ALTER TABLE assessments RENAME COLUMN profile_hash TO preferences_hash;
"#,
    )])
}

#[cfg(test)]
mod tests {
    #[test]
    fn migrations_are_valid() {
        super::migrations().validate().unwrap();
    }

    #[test]
    fn renamed_columns_carry_their_rows_and_constraints_forward() {
        // The rename is the risky half of migration 4: if SQLite did not
        // rewrite the UNIQUE constraint and the index with it, upserts would
        // start duplicating instead of replacing.
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        super::migrations().to_version(&mut conn, 3).unwrap();
        conn.execute_batch(
            r#"
INSERT INTO jobs (id, url, url_normalized, domain, content_text, source, status, created_at, updated_at)
  VALUES (1, 'u', 'u', 'd', 'text', 'extension', 'new', 'now', 'now');
INSERT INTO assessments (job_id, cv_hash, profile_hash, prompt_version, schema_version, facts_json, created_at)
  VALUES (1, 'abc', 'p1', 1, 1, '{}', 'now');
"#,
        )
        .unwrap();

        super::migrations().to_latest(&mut conn).unwrap();

        let (experience, preferences): (String, Option<String>) = conn
            .query_row(
                "SELECT experience_hash, preferences_hash FROM assessments WHERE job_id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(experience, "abc");
        assert_eq!(preferences.as_deref(), Some("p1"));

        // The UNIQUE constraint followed the rename.
        let dup = conn.execute(
            "INSERT INTO assessments (job_id, experience_hash, prompt_version, schema_version,
                                      facts_json, created_at)
             VALUES (1, 'abc', 1, 1, '{}', 'now')",
            [],
        );
        assert!(dup.is_err(), "UNIQUE(job_id, experience_hash, prompt_version) must survive");

        // And the dropped table is really gone.
        assert!(conn.prepare("SELECT 1 FROM cv_advice").is_err());
    }
}
