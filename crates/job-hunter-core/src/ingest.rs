//! Shared ingest path: the HTTP server and the stage-2 Google scraper both
//! feed scraped pages through `ingest_payload`, so dedupe and storage behave
//! identically regardless of source.

use crate::db::Db;
use crate::urlnorm;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct ScrapedPayload {
    pub url: String,
    #[serde(default)]
    pub title: Option<String>,
    pub text: String,
    #[serde(default)]
    pub html: Option<String>,
    #[serde(default = "default_source")]
    pub source: String,
}

fn default_source() -> String {
    "extension".to_string()
}

#[derive(Debug, Clone, Serialize)]
pub struct IngestResult {
    pub id: i64,
    pub duplicate: bool,
}

pub fn ingest_payload(db: &Db, payload: &ScrapedPayload) -> Result<IngestResult> {
    if payload.url.trim().is_empty() {
        bail!("url must not be empty");
    }
    if payload.text.trim().is_empty() {
        bail!("text must not be empty");
    }
    let url_normalized = urlnorm::normalize(&payload.url)?;
    let domain = urlnorm::domain(&payload.url)?;
    let title = payload
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let (id, duplicate) = db.insert_job(
        payload.url.trim(),
        &url_normalized,
        &domain,
        title,
        payload.text.trim(),
        payload.html.as_deref(),
        &payload.source,
    )?;
    Ok(IngestResult { id, duplicate })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_dedupes_url_variants() {
        let db = Db::open_in_memory().unwrap();
        let first = ingest_payload(
            &db,
            &ScrapedPayload {
                url: "https://example.com/jobs/1?utm_source=li#apply".into(),
                title: Some("Rust Dev".into()),
                text: "the job text".into(),
                html: None,
                source: "extension".into(),
            },
        )
        .unwrap();
        assert!(!first.duplicate);

        let second = ingest_payload(
            &db,
            &ScrapedPayload {
                url: "https://EXAMPLE.com/jobs/1".into(),
                title: None,
                text: "same job, different tracking".into(),
                html: None,
                source: "extension".into(),
            },
        )
        .unwrap();
        assert!(second.duplicate);
        assert_eq!(first.id, second.id);
    }

    #[test]
    fn rejects_empty_fields() {
        let db = Db::open_in_memory().unwrap();
        let bad = ScrapedPayload {
            url: "https://example.com".into(),
            title: None,
            text: "   ".into(),
            html: None,
            source: "extension".into(),
        };
        assert!(ingest_payload(&db, &bad).is_err());
    }
}
