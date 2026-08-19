//! What `cv-advice` asks the agent for: concrete, paste-ready CV changes for
//! one posting. Design rules mirror the assessment schema - closed shape,
//! additive evolution tracked by `schema_version`, unknown fields tolerated so
//! an old binary can still read a newer row.
//!
//! Nothing here feeds scoring. This is advice for a human to act on, stored so
//! `show` can render it next to the facts that produced it.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

pub const CURRENT_ADVICE_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CvAdvice {
    #[serde(default)]
    pub schema_version: i64,
    /// One or two sentences: how to frame this candidate for this role.
    #[serde(default)]
    pub positioning: Option<String>,
    #[serde(default)]
    pub rewrites: Vec<Rewrite>,
    /// CV material to lead with, expand, or move up.
    #[serde(default)]
    pub emphasise: Vec<String>,
    /// CV material to cut or shorten for this application.
    #[serde(default)]
    pub deemphasise: Vec<String>,
    #[serde(default)]
    pub gaps: Vec<Gap>,
    /// Terms the posting uses that the CV does not - what a keyword screen
    /// would miss.
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Rewrite {
    /// The CV section or heading to change, named as the CV names it.
    pub section: String,
    /// The current wording, quoted so the candidate can find it.
    #[serde(default)]
    pub current: Option<String>,
    /// Replacement text, ready to paste.
    pub suggested: String,
    #[serde(default)]
    pub why: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Gap {
    pub gap: String,
    #[serde(default)]
    pub how_to_address: Option<String>,
}

impl CvAdvice {
    /// Validation that drives the agent retry loop. Serde already guarantees
    /// that a rewrite has a section and suggested text; this rejects the
    /// answers that parse but say nothing actionable.
    pub fn validate(&self) -> Result<()> {
        if self.positioning.as_deref().unwrap_or("").trim().is_empty() {
            bail!("positioning must be a non-empty sentence or two");
        }
        if self.rewrites.is_empty() && self.emphasise.is_empty() {
            bail!("return at least one rewrite or one emphasise item");
        }
        for (i, r) in self.rewrites.iter().enumerate() {
            if r.section.trim().is_empty() {
                bail!("rewrites[{i}].section must name a CV section");
            }
            if r.suggested.trim().is_empty() {
                bail!("rewrites[{i}].suggested must contain the replacement text");
            }
        }
        Ok(())
    }
}

impl crate::agent::AgentJson for CvAdvice {
    fn check(&self) -> Result<()> {
        self.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{
        "schema_version": 1,
        "future_field": 1,
        "positioning": "A payments-infrastructure Rust engineer, not a generalist.",
        "rewrites": [{"section": "Summary", "current": "Senior software engineer",
                      "suggested": "Senior Rust engineer, payments infrastructure",
                      "why": "their first screen is for payments depth"}],
        "emphasise": ["the Payments Ltd rewrite"],
        "deemphasise": ["the 2017 internship"],
        "gaps": [{"gap": "Kafka", "how_to_address": "name the queue work you did instead"}],
        "keywords": ["event-driven", "PCI DSS"],
        "notes": "Nothing here needs invention."
    }"#;

    #[test]
    fn round_trips_and_tolerates_unknown_fields() {
        let advice: CvAdvice = serde_json::from_str(GOOD).unwrap();
        advice.validate().unwrap();
        assert_eq!(advice.rewrites[0].section, "Summary");
        assert_eq!(advice.keywords, vec!["event-driven", "PCI DSS"]);

        let again: CvAdvice = serde_json::from_str(&serde_json::to_string(&advice).unwrap()).unwrap();
        assert_eq!(again.gaps[0].gap, "Kafka");
    }

    #[test]
    fn empty_or_hollow_answers_are_rejected() {
        let advice: CvAdvice = serde_json::from_str(r#"{"rewrites": []}"#).unwrap();
        assert!(advice.validate().is_err(), "no positioning");

        let advice: CvAdvice =
            serde_json::from_str(r#"{"positioning": "Lead with Rust."}"#).unwrap();
        assert!(advice.validate().is_err(), "nothing actionable");

        let advice: CvAdvice = serde_json::from_str(
            r#"{"positioning": "Lead with Rust.",
                "rewrites": [{"section": " ", "suggested": "x"}]}"#,
        )
        .unwrap();
        assert!(advice.validate().is_err(), "unnamed section");
    }

    #[test]
    fn a_rewrite_without_replacement_text_fails_to_parse() {
        // Structure the agent must get right; the runner retries with the error.
        assert!(serde_json::from_str::<CvAdvice>(r#"{"rewrites": [{"section": "Summary"}]}"#).is_err());
    }
}
