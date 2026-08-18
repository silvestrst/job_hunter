//! Shared ingest path: the HTTP server and the stage-2 Google scraper both
//! feed scraped pages through `ingest_payload`, so dedupe and storage behave
//! identically regardless of source.

use crate::db::Db;
use crate::{cleaner, urlnorm};
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
    let text = payload.text.trim();
    let content_text = cleaner::clean(&domain, text).unwrap_or_else(|| text.to_string());
    let (id, duplicate) = db.insert_job(
        payload.url.trim(),
        &url_normalized,
        &domain,
        title,
        &content_text,
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
    fn linkedin_text_is_cleaned_before_storage() {
        let db = Db::open_in_memory().unwrap();
        let description = "You will design and operate low-latency trading systems in Rust, \
            owning exchange connectivity and market data components end to end while \
            collaborating with quants and traders across a fully remote global team.";
        let page = format!(
            "Home\nMy Network\nJobs\n\nKeyrock\n\nRust Engineer\n\n\
             London · 3 weeks ago · 13 people clicked apply\n\nRemote\nApply\n\n\
             About the job\n\n{description}\n\nSet alert for similar jobs\n\
             LinkedIn Corporation © 2026\n"
        );
        let result = ingest_payload(
            &db,
            &ScrapedPayload {
                url: "https://www.linkedin.com/jobs/view/12345/".into(),
                title: Some("Rust Engineer".into()),
                text: page,
                html: None,
                source: "extension".into(),
            },
        )
        .unwrap();
        let stored = db.get_job(result.id).unwrap().content_text;
        assert!(stored.starts_with("Keyrock\n\nRust Engineer\n"));
        assert!(stored.contains(description));
        assert!(!stored.contains("My Network"));
        assert!(!stored.contains("Set alert for similar jobs"));
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
