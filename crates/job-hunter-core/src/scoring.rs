//! Layer-2: pure logic over stored assessment facts. No LLM anywhere in this
//! module - changing criteria re-runs this, never the agent.
//!
//! Two separate jobs, deliberately not mixed:
//!
//! * **The score** answers one question - how well does this candidate fit
//!   this role - and is the mean of the two fit measures the agent produced,
//!   `overall_cv_match` and `domain_familiarity`, on a 0-100 scale. Nothing
//!   else moves it. Salary, seniority, work mode and the rest are facts about
//!   the posting, not measures of the candidate, so they are reported and
//!   filtered on, never weighted in.
//! * **The filters** answer a different question - is this role worth looking
//!   at at all - and are pass/fail only. There is no grading here: a filter
//!   either lets a job through or it doesn't.

use crate::assessment::schema::{AssessmentFacts, SalaryPeriod};
use crate::config::{CriteriaConfig, FilterOp, HardFilter, MissingPolicy};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// Typed reference to an assessment fact, parsed from the YAML `fact:` string.
/// Typos are caught by `config check`, not at scoring time.
#[derive(Debug, Clone, PartialEq)]
pub enum FactRef {
    SalaryMaxGbp,
    SalaryMinGbp,
    WorkMode,
    EmploymentType,
    SeniorityLevel,
    SeniorityFit,
    OverallCvMatch,
    DomainFamiliarity,
    PreferenceMatch,
    VisaSponsorship,
    Country,
    Skill(String),
}

pub const VALID_FACT_NAMES: &str = "salary_max_gbp, salary_min_gbp, work_mode, employment_type, \
     seniority_level, seniority_fit, overall_cv_match, domain_familiarity, preference_match, \
     visa_sponsorship, country, skill:<name>";

impl FactRef {
    pub fn parse(name: &str) -> Result<Self> {
        if let Some(skill) = name.strip_prefix("skill:") {
            if skill.trim().is_empty() {
                bail!("skill: fact needs a name, e.g. skill:rust");
            }
            return Ok(FactRef::Skill(skill.trim().to_ascii_lowercase()));
        }
        Ok(match name {
            "salary_max_gbp" => FactRef::SalaryMaxGbp,
            "salary_min_gbp" => FactRef::SalaryMinGbp,
            "work_mode" => FactRef::WorkMode,
            "employment_type" => FactRef::EmploymentType,
            "seniority_level" => FactRef::SeniorityLevel,
            "seniority_fit" => FactRef::SeniorityFit,
            "overall_cv_match" => FactRef::OverallCvMatch,
            "domain_familiarity" => FactRef::DomainFamiliarity,
            "preference_match" => FactRef::PreferenceMatch,
            "visa_sponsorship" => FactRef::VisaSponsorship,
            "country" => FactRef::Country,
            other => bail!("unknown fact '{other}' (valid: {VALID_FACT_NAMES})"),
        })
    }
}

/// A resolved fact value: number, text (enum values as strings), or missing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FactValue {
    Number(f64),
    Text(String),
    Missing,
}

fn period_multiplier(period: SalaryPeriod) -> f64 {
    match period {
        SalaryPeriod::Day => 220.0,
        SalaryPeriod::Hour => 1840.0,
        // Yearly stated or unstated: assume annual.
        SalaryPeriod::Year | SalaryPeriod::Unspecified => 1.0,
    }
}

fn salary_gbp(facts: &AssessmentFacts, criteria: &CriteriaConfig, prefer_max: bool) -> FactValue {
    let salary = &facts.extracted.salary;
    let amount = if prefer_max {
        salary.max.or(salary.min)
    } else {
        salary.min.or(salary.max)
    };
    let Some(amount) = amount else {
        return FactValue::Missing;
    };
    let rate = match salary.currency.as_deref() {
        None => 1.0, // no currency stated: assume GBP (the user's market)
        Some(code) => {
            let code = code.trim().to_ascii_uppercase();
            if code == "GBP" || code.is_empty() {
                1.0
            } else {
                match criteria.currency_rates_to_gbp.get(&code) {
                    Some(rate) => *rate,
                    None => return FactValue::Missing, // unknown currency: can't compare honestly
                }
            }
        }
    };
    FactValue::Number(amount * rate * period_multiplier(salary.period))
}

fn text_or_missing(s: &str) -> FactValue {
    if s == "unspecified" {
        FactValue::Missing
    } else {
        FactValue::Text(s.to_string())
    }
}

pub fn resolve(fact: &FactRef, facts: &AssessmentFacts, criteria: &CriteriaConfig) -> FactValue {
    match fact {
        FactRef::SalaryMaxGbp => salary_gbp(facts, criteria, true),
        FactRef::SalaryMinGbp => salary_gbp(facts, criteria, false),
        FactRef::WorkMode => text_or_missing(facts.extracted.work_mode.as_str()),
        FactRef::EmploymentType => text_or_missing(facts.extracted.employment_type.as_str()),
        FactRef::SeniorityLevel => text_or_missing(facts.extracted.seniority_level.as_str()),
        FactRef::SeniorityFit => text_or_missing(facts.fit.seniority_fit.as_str()),
        FactRef::OverallCvMatch => facts
            .fit
            .overall_cv_match
            .map_or(FactValue::Missing, FactValue::Number),
        FactRef::DomainFamiliarity => facts
            .fit
            .domain_familiarity
            .map_or(FactValue::Missing, FactValue::Number),
        FactRef::PreferenceMatch => text_or_missing(facts.fit.preference_match.as_str()),
        FactRef::VisaSponsorship => text_or_missing(facts.extracted.visa_sponsorship.as_str()),
        FactRef::Country => match &facts.extracted.country {
            Some(c) if !c.trim().is_empty() => FactValue::Text(c.trim().to_ascii_lowercase()),
            _ => FactValue::Missing,
        },
        FactRef::Skill(name) => {
            let found = facts
                .fit
                .skill_matches
                .iter()
                .find(|m| m.skill.eq_ignore_ascii_case(name));
            match found {
                Some(m) => FactValue::Text(m.cv_evidence.as_str().to_string()),
                None => FactValue::Missing,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOutcome {
    Pass,
    Fail,
    MissingPass,
    MissingFail,
}

impl FilterOutcome {
    pub fn passed(&self) -> bool {
        matches!(self, FilterOutcome::Pass | FilterOutcome::MissingPass)
    }
}

fn value_matches(actual: &FactValue, expected: &serde_json::Value) -> bool {
    match (actual, expected) {
        (FactValue::Number(n), v) => v.as_f64() == Some(*n),
        (FactValue::Text(t), serde_json::Value::String(s)) => t.eq_ignore_ascii_case(s),
        _ => false,
    }
}

fn eval_filter(filter: &HardFilter, actual: &FactValue) -> FilterOutcome {
    if *actual == FactValue::Missing {
        return match filter.if_missing {
            MissingPolicy::Pass => FilterOutcome::MissingPass,
            MissingPolicy::Fail => FilterOutcome::MissingFail,
        };
    }
    let pass = match filter.op {
        FilterOp::Gte => matches!(actual, FactValue::Number(n) if filter.value.as_f64().is_some_and(|v| *n >= v)),
        FilterOp::Lte => matches!(actual, FactValue::Number(n) if filter.value.as_f64().is_some_and(|v| *n <= v)),
        FilterOp::Eq => value_matches(actual, &filter.value),
        FilterOp::In => filter
            .value
            .as_array()
            .is_some_and(|arr| arr.iter().any(|v| value_matches(actual, v))),
        FilterOp::NotIn => filter
            .value
            .as_array()
            .is_some_and(|arr| !arr.iter().any(|v| value_matches(actual, v))),
    };
    if pass {
        FilterOutcome::Pass
    } else {
        FilterOutcome::Fail
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterBreakdown {
    pub fact: String,
    pub op: FilterOp,
    pub value: serde_json::Value,
    pub actual: FactValue,
    pub outcome: FilterOutcome,
}

/// The score and what it was made of. `cv_match` and `domain_familiarity` are
/// the agent's 0..1 measures restated as percentages, and `score` is their
/// mean - so the three numbers always visibly add up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreResult {
    pub score: f64,
    /// None when the agent didn't produce the measure.
    #[serde(default)]
    pub cv_match: Option<f64>,
    #[serde(default)]
    pub domain_familiarity: Option<f64>,
    pub hard_filter_passed: bool,
    #[serde(default)]
    pub filters: Vec<FilterBreakdown>,
}

/// The two fit measures as percentages, and their mean.
///
/// A measure the agent omitted is left out of the mean rather than counted as
/// zero: a missing number is an absence of evidence, and averaging it in as 0
/// would report a confident bad fit the assessment never claimed. With both
/// absent there is nothing to average and the score is 0 - the job sorts to
/// the bottom, where an unassessable posting belongs.
fn fit_score(facts: &AssessmentFacts) -> (f64, Option<f64>, Option<f64>) {
    let cv_match = facts.fit.overall_cv_match.map(|v| v.clamp(0.0, 1.0) * 100.0);
    let domain = facts.fit.domain_familiarity.map(|v| v.clamp(0.0, 1.0) * 100.0);
    let present: Vec<f64> = [cv_match, domain].into_iter().flatten().collect();
    let score = if present.is_empty() {
        0.0
    } else {
        present.iter().sum::<f64>() / present.len() as f64
    };
    (score, cv_match, domain)
}

/// Score one job's facts and run the hard filters over them. Criteria are
/// assumed validated (`Config::validate`); an unparseable fact here would be a
/// bug, so it fails loudly.
///
/// The two halves are independent by design: the filters decide whether a job
/// is shown, the fit measures decide how it ranks among those shown. A job
/// that fails a filter still gets its real score, so `--include-filtered`
/// shows a ranking rather than a pile of zeroes.
pub fn compute_score(criteria: &CriteriaConfig, facts: &AssessmentFacts) -> Result<ScoreResult> {
    let mut filters = Vec::new();
    let mut hard_filter_passed = true;
    for f in &criteria.hard_filters {
        let fact = FactRef::parse(&f.fact)?;
        let actual = resolve(&fact, facts, criteria);
        let outcome = eval_filter(f, &actual);
        hard_filter_passed &= outcome.passed();
        filters.push(FilterBreakdown {
            fact: f.fact.clone(),
            op: f.op,
            value: f.value.clone(),
            actual,
            outcome,
        });
    }

    let (score, cv_match, domain_familiarity) = fit_score(facts);
    Ok(ScoreResult {
        score,
        cv_match,
        domain_familiarity,
        hard_filter_passed,
        filters,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn sample_facts() -> AssessmentFacts {
        serde_json::from_str(
            r#"{
            "schema_version": 3,
            "extracted": {
                "work_mode": "remote",
                "salary": {"min": 90000, "max": 110000, "currency": "GBP", "period": "year"},
                "seniority_level": "senior"
            },
            "fit": {
                "skill_matches": [
                    {"skill": "Rust", "cv_evidence": "strong", "relevance": 0.9},
                    {"skill": "kubernetes", "cv_evidence": "weak", "relevance": 0.4}
                ],
                "seniority_fit": "match",
                "domain_familiarity": 0.5,
                "overall_cv_match": 0.8,
                "preference_match": "good"
            }
        }"#,
        )
        .unwrap()
    }

    fn default_criteria() -> CriteriaConfig {
        let config: Config = serde_yaml::from_str(crate::config::DEFAULT_CONFIG_YAML).unwrap();
        config.criteria
    }

    #[test]
    fn score_is_the_mean_of_the_two_fit_measures() {
        let result = compute_score(&default_criteria(), &sample_facts()).unwrap();
        assert!(result.hard_filter_passed);
        assert_eq!(result.cv_match, Some(80.0));
        assert_eq!(result.domain_familiarity, Some(50.0));
        assert_eq!(result.score, 65.0);
    }

    #[test]
    fn nothing_but_the_two_fit_measures_moves_the_score() {
        // Every other fact swung to its worst plausible value; the score is
        // the one thing that must not budge.
        let baseline = compute_score(&default_criteria(), &sample_facts()).unwrap().score;
        let mut facts = sample_facts();
        facts.extracted.work_mode = crate::assessment::schema::WorkMode::Onsite;
        facts.extracted.salary = Default::default();
        facts.fit.seniority_fit = crate::assessment::schema::SeniorityFit::Under;
        facts.fit.preference_match = crate::assessment::schema::PreferenceMatch::Poor;
        facts.fit.skill_matches.clear();
        let result = compute_score(&default_criteria(), &facts).unwrap();
        assert_eq!(result.score, baseline);
        assert!(!result.hard_filter_passed, "the same facts still fail the filters");
    }

    #[test]
    fn a_missing_measure_is_left_out_of_the_mean() {
        let mut facts = sample_facts();
        facts.fit.domain_familiarity = None;
        let result = compute_score(&default_criteria(), &facts).unwrap();
        assert_eq!(result.score, 80.0, "the CV match alone, not halved");
        assert_eq!(result.domain_familiarity, None);

        facts.fit.overall_cv_match = None;
        let result = compute_score(&default_criteria(), &facts).unwrap();
        assert_eq!(result.score, 0.0, "nothing to average");
    }

    #[test]
    fn hard_filter_fails_onsite_without_touching_the_score() {
        let mut facts = sample_facts();
        facts.extracted.work_mode = crate::assessment::schema::WorkMode::Onsite;
        let result = compute_score(&default_criteria(), &facts).unwrap();
        assert!(!result.hard_filter_passed);
        assert_eq!(result.score, 65.0, "filters gate visibility, not rank");
    }

    #[test]
    fn missing_salary_respects_policy() {
        let mut facts = sample_facts();
        facts.extracted.salary = Default::default();
        let criteria = default_criteria();
        let result = compute_score(&criteria, &facts).unwrap();
        // salary filter has if_missing: pass
        assert!(result.hard_filter_passed);
        assert_eq!(result.filters[0].outcome, FilterOutcome::MissingPass);
    }

    #[test]
    fn currency_conversion_applies() {
        let mut facts = sample_facts();
        facts.extracted.salary.currency = Some("USD".into());
        facts.extracted.salary.max = Some(100_000.0);
        let criteria = default_criteria();
        let result = compute_score(&criteria, &facts).unwrap();
        // 100k USD * 0.78 = 78k GBP < 85k threshold -> fail
        assert!(!result.hard_filter_passed);
    }

    #[test]
    fn unknown_currency_is_missing() {
        let mut facts = sample_facts();
        facts.extracted.salary.currency = Some("XYZ".into());
        let value = resolve(&FactRef::SalaryMaxGbp, &facts, &default_criteria());
        assert_eq!(value, FactValue::Missing);
    }

    #[test]
    fn day_rate_annualizes() {
        let mut facts = sample_facts();
        facts.extracted.salary.max = Some(500.0);
        facts.extracted.salary.min = None;
        facts.extracted.salary.period = crate::assessment::schema::SalaryPeriod::Day;
        let value = resolve(&FactRef::SalaryMaxGbp, &facts, &default_criteria());
        assert_eq!(value, FactValue::Number(110_000.0));
    }

    #[test]
    fn preference_match_is_filterable_but_unscored() {
        let facts = sample_facts();
        let fact = FactRef::parse("preference_match").unwrap();
        assert_eq!(resolve(&fact, &facts, &default_criteria()), FactValue::Text("good".into()));

        let filter = HardFilter {
            fact: "preference_match".into(),
            op: FilterOp::NotIn,
            value: serde_json::json!(["poor"]),
            if_missing: MissingPolicy::Pass,
        };
        let criteria = CriteriaConfig { hard_filters: vec![filter], ..default_criteria() };
        assert!(compute_score(&criteria, &facts).unwrap().hard_filter_passed);

        let mut poor = facts.clone();
        poor.fit.preference_match = crate::assessment::schema::PreferenceMatch::Poor;
        let result = compute_score(&criteria, &poor).unwrap();
        assert!(!result.hard_filter_passed);
        assert_eq!(result.score, 65.0, "a preference verdict never moves the score");
    }

    #[test]
    fn unassessed_preferences_pass_an_if_missing_pass_filter() {
        // No preferences document means "unspecified", which must read as
        // absent rather than as a bad grade.
        let mut facts = sample_facts();
        facts.fit.preference_match = crate::assessment::schema::PreferenceMatch::Unspecified;
        let criteria = CriteriaConfig {
            hard_filters: vec![HardFilter {
                fact: "preference_match".into(),
                op: FilterOp::In,
                value: serde_json::json!(["strong", "exceptional"]),
                if_missing: MissingPolicy::Pass,
            }],
            ..default_criteria()
        };
        assert!(compute_score(&criteria, &facts).unwrap().hard_filter_passed);
    }
}
