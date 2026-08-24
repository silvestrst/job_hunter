//! The generic assessment facts the LLM must return. Design rules: extracted
//! facts, normalized units, closed enums, and CV-relative measures only —
//! never criteria-relative judgments. Unknown values are explicit
//! ("unspecified"/null) and never guessed - except salary, which the LLM is
//! prompted to estimate from role/seniority/location/tech-stack when unstated
//! (see `Salary::is_estimate`). Evolution is additive-only, tracked by
//! `schema_version`; unknown JSON fields are tolerated on deserialize so old
//! binaries can read newer rows.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

pub const CURRENT_SCHEMA_VERSION: i64 = 3;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AssessmentFacts {
    #[serde(default)]
    pub schema_version: i64,
    #[serde(default)]
    pub extracted: Extracted,
    #[serde(default)]
    pub fit: Fit,
    #[serde(default)]
    pub confidence: Confidence,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Extracted {
    #[serde(default)]
    pub job_title: Option<String>,
    #[serde(default)]
    pub company: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    /// ISO 3166-1 alpha-2, e.g. "GB".
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub work_mode: WorkMode,
    #[serde(default)]
    pub hybrid_days_in_office: Option<i64>,
    #[serde(default)]
    pub remote_restriction: Option<String>,
    #[serde(default)]
    pub salary: Salary,
    #[serde(default)]
    pub employment_type: EmploymentType,
    #[serde(default)]
    pub seniority_level: SeniorityLevel,
    #[serde(default)]
    pub visa_sponsorship: VisaSponsorship,
    #[serde(default)]
    pub tech_stack: Vec<String>,
    #[serde(default)]
    pub industry: Option<String>,
    /// ISO date if stated in the posting.
    #[serde(default)]
    pub posted_date: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Salary {
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    /// ISO 4217, e.g. "GBP".
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub period: SalaryPeriod,
    /// True when the posting didn't state a salary and the LLM inferred this
    /// range from role/seniority/location/tech-stack instead.
    #[serde(default)]
    pub is_estimate: bool,
    #[serde(default)]
    pub raw_text: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Fit {
    #[serde(default)]
    pub skill_matches: Vec<SkillMatch>,
    #[serde(default)]
    pub skills_missing: Vec<String>,
    #[serde(default)]
    pub seniority_fit: SeniorityFit,
    /// 0..1 vs the experience document's industry history. Half the score.
    #[serde(default)]
    pub domain_familiarity: Option<f64>,
    /// 0..1 holistic experience-to-role match. The other half of the score.
    #[serde(default)]
    pub overall_cv_match: Option<f64>,
    /// Holistic verdict on the posting against what the candidate said they
    /// want, as one closed grade. Advisory: it is reported, never scored, and
    /// is "unspecified" when no preferences document was supplied.
    #[serde(default)]
    pub preference_match: PreferenceMatch,
    /// One paragraph explaining `preference_match` - which wants this posting
    /// meets, which it misses. Null when no preferences document was supplied.
    #[serde(default)]
    pub preference_summary: Option<String>,
    #[serde(default)]
    pub red_flags: Vec<String>,
    #[serde(default)]
    pub green_flags: Vec<String>,
    /// Actionable, experience-specific advice for applying to this role - not
    /// used by scoring, shown to the candidate as-is.
    #[serde(default)]
    pub suggestions: Suggestions,
    #[serde(default)]
    pub notes: Option<String>,
}

/// How to approach this application: the reasoning in prose, then the two
/// lists that reasoning rests on. Three fields rather than one blob because
/// the shape is what makes the agent produce all three parts - asked for free
/// text, models reliably write the paragraphs and skip the breakdowns.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Suggestions {
    /// One or two paragraphs: what to foreground, what to preempt, what to
    /// ask. None when the agent had nothing worth saying.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advice: Option<String>,
    /// What this posting wants that the experience doesn't show.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<String>,
    /// What the candidate can lead with.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub strengths: Vec<String>,
}

impl Suggestions {
    pub fn is_empty(&self) -> bool {
        self.advice.as_ref().is_none_or(|a| a.trim().is_empty())
            && self.gaps.is_empty()
            && self.strengths.is_empty()
    }
}

/// Accepts the current object and the bare array of strings that schema 2
/// used, so `show` still renders assessments stored before the split rather
/// than failing to parse them. The old items were discrete pieces of advice,
/// which is what `advice` now holds.
impl<'de> Deserialize<'de> for Suggestions {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Structured {
                #[serde(default)]
                advice: Option<String>,
                #[serde(default)]
                gaps: Vec<String>,
                #[serde(default)]
                strengths: Vec<String>,
            },
            Legacy(Vec<String>),
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Structured { advice, gaps, strengths } => Self { advice, gaps, strengths },
            Repr::Legacy(items) => Self {
                advice: (!items.is_empty()).then(|| items.join("\n\n")),
                ..Default::default()
            },
        })
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SkillMatch {
    pub skill: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub years_required: Option<f64>,
    #[serde(default)]
    pub cv_evidence: CvEvidence,
    /// 0..1 importance of this skill to the role.
    #[serde(default)]
    pub relevance: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Confidence {
    #[serde(default)]
    pub salary: Option<f64>,
    #[serde(default)]
    pub work_mode: Option<f64>,
    #[serde(default)]
    pub overall: Option<f64>,
}

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant,)+
            /// Catch-all: absent, "unspecified", or any unrecognized value.
            #[default]
            #[serde(other, rename = "unspecified")]
            Unspecified,
        }

        impl $name {
            pub fn as_str(&self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                    Self::Unspecified => "unspecified",
                }
            }
        }
    };
}

string_enum!(WorkMode {
    Remote => "remote",
    Hybrid => "hybrid",
    Onsite => "onsite",
});

string_enum!(SalaryPeriod {
    Year => "year",
    Day => "day",
    Hour => "hour",
});

string_enum!(EmploymentType {
    Permanent => "permanent",
    Contract => "contract",
    Freelance => "freelance",
    Internship => "internship",
});

string_enum!(SeniorityLevel {
    Junior => "junior",
    Mid => "mid",
    Senior => "senior",
    Staff => "staff",
    Principal => "principal",
    Lead => "lead",
    Manager => "manager",
});

string_enum!(VisaSponsorship {
    Yes => "yes",
    No => "no",
});

string_enum!(SeniorityFit {
    Under => "under",
    Match => "match",
    Over => "over",
});

string_enum!(CvEvidence {
    Strong => "strong",
    Moderate => "moderate",
    Weak => "weak",
    None => "none",
});

// Ordered worst to best; the order is the display order, nothing scores off it.
string_enum!(PreferenceMatch {
    Poor => "poor",
    Average => "average",
    Good => "good",
    Strong => "strong",
    Exceptional => "exceptional",
});

fn check_unit(name: &str, value: Option<f64>) -> Result<()> {
    if let Some(v) = value {
        if !(0.0..=1.0).contains(&v) {
            bail!("{name} must be within 0..1, got {v}");
        }
    }
    Ok(())
}

impl AssessmentFacts {
    /// Semantic validation used to drive the agent retry loop: serde already
    /// guarantees shape; this checks value ranges.
    pub fn validate(&self) -> Result<()> {
        check_unit("fit.domain_familiarity", self.fit.domain_familiarity)?;
        check_unit("fit.overall_cv_match", self.fit.overall_cv_match)?;
        for m in &self.fit.skill_matches {
            check_unit(&format!("fit.skill_matches[{}].relevance", m.skill), m.relevance)?;
        }
        check_unit("confidence.salary", self.confidence.salary)?;
        check_unit("confidence.work_mode", self.confidence.work_mode)?;
        check_unit("confidence.overall", self.confidence.overall)?;
        if let (Some(min), Some(max)) = (self.extracted.salary.min, self.extracted.salary.max) {
            if min > max {
                bail!("extracted.salary.min ({min}) exceeds max ({max})");
            }
        }
        Ok(())
    }
}

impl crate::agent::AgentJson for AssessmentFacts {
    fn check(&self) -> Result<()> {
        self.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_unknown_fields() {
        let json = r#"{
            "schema_version": 1,
            "future_field": {"x": 1},
            "extracted": {
                "job_title": "Senior Rust Engineer",
                "company": "Acme Ltd",
                "work_mode": "remote",
                "salary": {"min": 85000, "max": 100000, "currency": "GBP", "period": "year"},
                "seniority_level": "senior",
                "unknown_extracted_field": true
            },
            "fit": {
                "skill_matches": [{"skill": "rust", "required": true, "cv_evidence": "strong", "relevance": 0.95}],
                "seniority_fit": "match",
                "overall_cv_match": 0.82
            }
        }"#;
        let facts: AssessmentFacts = serde_json::from_str(json).unwrap();
        facts.validate().unwrap();
        assert_eq!(facts.extracted.work_mode, WorkMode::Remote);
        assert_eq!(facts.fit.skill_matches[0].cv_evidence, CvEvidence::Strong);

        let out = serde_json::to_string(&facts).unwrap();
        let again: AssessmentFacts = serde_json::from_str(&out).unwrap();
        assert_eq!(again.extracted.salary.max, Some(100000.0));
    }

    #[test]
    fn suggestions_round_trip() {
        let json = r#"{"fit": {"suggestions": {
            "advice": "Lead with the payments work.\n\nAsk about on-call.",
            "gaps": ["no Kafka"],
            "strengths": ["production Rust"]
        }}}"#;
        let facts: AssessmentFacts = serde_json::from_str(json).unwrap();
        let s = &facts.fit.suggestions;
        assert!(s.advice.as_deref().unwrap().contains("payments work"));
        assert_eq!(s.gaps, vec!["no Kafka"]);
        assert_eq!(s.strengths, vec!["production Rust"]);
        assert!(!s.is_empty());

        let again: AssessmentFacts =
            serde_json::from_str(&serde_json::to_string(&facts).unwrap()).unwrap();
        assert_eq!(again.fit.suggestions.gaps, s.gaps);
        assert_eq!(again.fit.suggestions.advice, s.advice);
    }

    #[test]
    fn schema_2_suggestions_still_parse() {
        // Assessments stored as a bare array predate the split; `show` must
        // still render them rather than failing on the whole facts blob.
        let json = r#"{"fit": {"suggestions": ["highlight your Rust experience", "ask about on-call"]}}"#;
        let facts: AssessmentFacts = serde_json::from_str(json).unwrap();
        let advice = facts.fit.suggestions.advice.unwrap();
        assert!(advice.contains("highlight your Rust experience"));
        assert!(advice.contains("ask about on-call"));
        assert!(facts.fit.suggestions.gaps.is_empty());
    }

    #[test]
    fn absent_suggestions_are_empty_not_missing() {
        let facts: AssessmentFacts = serde_json::from_str(r#"{"fit": {}}"#).unwrap();
        assert!(facts.fit.suggestions.is_empty());
        // Whitespace-only advice is nothing to show either.
        let facts: AssessmentFacts =
            serde_json::from_str(r#"{"fit": {"suggestions": {"advice": "  "}}}"#).unwrap();
        assert!(facts.fit.suggestions.is_empty());
    }

    #[test]
    fn unknown_enum_value_becomes_unspecified() {
        let facts: AssessmentFacts =
            serde_json::from_str(r#"{"extracted": {"work_mode": "wework-only"}}"#).unwrap();
        assert_eq!(facts.extracted.work_mode, WorkMode::Unspecified);
    }

    #[test]
    fn preference_match_defaults_to_unspecified() {
        // No preferences document -> the agent leaves both fields out, and
        // that must read as "not assessed", not as a bad grade.
        let facts: AssessmentFacts = serde_json::from_str(r#"{"fit": {}}"#).unwrap();
        assert_eq!(facts.fit.preference_match, PreferenceMatch::Unspecified);
        assert!(facts.fit.preference_summary.is_none());

        let facts: AssessmentFacts = serde_json::from_str(
            r#"{"fit": {"preference_match": "strong", "preference_summary": "Remote and no on-call."}}"#,
        )
        .unwrap();
        assert_eq!(facts.fit.preference_match, PreferenceMatch::Strong);
        assert_eq!(facts.fit.preference_summary.as_deref(), Some("Remote and no on-call."));
    }

    #[test]
    fn out_of_range_rejected() {
        let facts: AssessmentFacts =
            serde_json::from_str(r#"{"fit": {"overall_cv_match": 1.4}}"#).unwrap();
        assert!(facts.validate().is_err());
    }
}
