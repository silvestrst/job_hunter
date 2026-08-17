//! Layer-2 scoring: pure logic over stored assessment facts. No LLM anywhere
//! in this module — changing criteria re-runs this, never the agent.

use crate::assessment::schema::{AssessmentFacts, CvEvidence, SalaryPeriod, SeniorityFit};
use crate::config::{CriteriaConfig, FilterOp, HardFilter, MissingPolicy, SoftCriterion};
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
    VisaSponsorship,
    Country,
    Skill(String),
}

pub const VALID_FACT_NAMES: &str = "salary_max_gbp, salary_min_gbp, work_mode, employment_type, \
     seniority_level, seniority_fit, overall_cv_match, domain_familiarity, visa_sponsorship, \
     country, skill:<name>";

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
            "visa_sponsorship" => FactRef::VisaSponsorship,
            "country" => FactRef::Country,
            other => bail!("unknown fact '{other}' (valid: {VALID_FACT_NAMES})"),
        })
    }

    /// Validation used by `config check`: soft criteria must be scoreable.
    pub fn check_soft(&self, criterion: &SoftCriterion) -> Result<()> {
        match self {
            FactRef::SalaryMaxGbp | FactRef::SalaryMinGbp => {
                if criterion.target.is_none() {
                    bail!("numeric fact '{}' needs a 'target' when used as a soft criterion", criterion.fact);
                }
            }
            FactRef::WorkMode
            | FactRef::EmploymentType
            | FactRef::SeniorityLevel
            | FactRef::VisaSponsorship
            | FactRef::Country => {
                if criterion.map.is_none() {
                    bail!(
                        "enum fact '{}' needs a 'map' (value -> sub-score) when used as a soft criterion",
                        criterion.fact
                    );
                }
            }
            // seniority_fit and skill:<name> have built-in mappings;
            // overall_cv_match / domain_familiarity are already 0..1.
            _ => {}
        }
        Ok(())
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

fn builtin_enum_score(fact: &FactRef, text: &str) -> Option<f64> {
    match fact {
        FactRef::SeniorityFit => Some(match text {
            t if t == SeniorityFit::Match.as_str() => 1.0,
            t if t == SeniorityFit::Over.as_str() => 0.5,
            t if t == SeniorityFit::Under.as_str() => 0.2,
            _ => 0.5,
        }),
        FactRef::Skill(_) => Some(match text {
            t if t == CvEvidence::Strong.as_str() => 1.0,
            t if t == CvEvidence::Moderate.as_str() => 0.6,
            t if t == CvEvidence::Weak.as_str() => 0.3,
            _ => 0.0,
        }),
        _ => None,
    }
}

fn sub_score(criterion: &SoftCriterion, fact: &FactRef, value: &FactValue) -> f64 {
    let missing_score = criterion.missing_score.unwrap_or(0.0);
    match value {
        FactValue::Missing => missing_score,
        FactValue::Number(n) => match criterion.target {
            Some(target) if target > 0.0 => (n / target).clamp(0.0, 1.0),
            _ => n.clamp(0.0, 1.0),
        },
        FactValue::Text(t) => {
            if let Some(map) = &criterion.map {
                map.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(t))
                    .map(|(_, v)| v.clamp(0.0, 1.0))
                    .unwrap_or(missing_score)
            } else {
                builtin_enum_score(fact, t).unwrap_or(missing_score)
            }
        }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoftBreakdown {
    pub fact: String,
    pub weight: f64,
    pub value: FactValue,
    pub sub_score: f64,
    /// Points contributed to the 0..100 total.
    pub contribution: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreResult {
    pub score: f64,
    pub hard_filter_passed: bool,
    pub filters: Vec<FilterBreakdown>,
    pub soft: Vec<SoftBreakdown>,
}

/// Compute the layer-2 score for one job's facts under the given criteria.
/// Criteria are assumed validated (`Config::validate`); an unparseable fact
/// here would be a bug, so it fails loudly.
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

    let total_weight: f64 = criteria.soft.iter().map(|s| s.weight).sum();
    let mut soft = Vec::new();
    let mut score = 0.0;
    for s in &criteria.soft {
        let fact = FactRef::parse(&s.fact)?;
        let value = resolve(&fact, facts, criteria);
        let sub = sub_score(s, &fact, &value);
        let contribution = if total_weight > 0.0 {
            100.0 * s.weight * sub / total_weight
        } else {
            0.0
        };
        score += contribution;
        soft.push(SoftBreakdown {
            fact: s.fact.clone(),
            weight: s.weight,
            value,
            sub_score: sub,
            contribution,
        });
    }

    Ok(ScoreResult {
        score,
        hard_filter_passed,
        filters,
        soft,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn sample_facts() -> AssessmentFacts {
        serde_json::from_str(
            r#"{
            "schema_version": 1,
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
                "overall_cv_match": 0.8
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
    fn scores_sample_facts() {
        let result = compute_score(&default_criteria(), &sample_facts()).unwrap();
        assert!(result.hard_filter_passed);
        // Weights: cv_match 5*0.8, seniority 3*1.0, salary(target 120k) 3*(110/120), domain 1*0.5
        let expected = 100.0 * (5.0 * 0.8 + 3.0 * 1.0 + 3.0 * (110_000.0 / 120_000.0) + 1.0 * 0.5) / 12.0;
        assert!((result.score - expected).abs() < 1e-6, "got {}", result.score);
    }

    #[test]
    fn hard_filter_fails_onsite() {
        let mut facts = sample_facts();
        facts.extracted.work_mode = crate::assessment::schema::WorkMode::Onsite;
        let result = compute_score(&default_criteria(), &facts).unwrap();
        assert!(!result.hard_filter_passed);
        // Soft score still computed for visibility.
        assert!(result.score > 0.0);
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
    fn skill_fact_uses_cv_evidence() {
        let criteria = default_criteria();
        let facts = sample_facts();
        let criterion = SoftCriterion {
            fact: "skill:rust".into(),
            weight: 1.0,
            target: None,
            map: None,
            missing_score: None,
        };
        let fact = FactRef::parse("skill:rust").unwrap();
        let value = resolve(&fact, &facts, &criteria);
        assert_eq!(sub_score(&criterion, &fact, &value), 1.0);

        let missing = FactRef::parse("skill:cobol").unwrap();
        let value = resolve(&missing, &facts, &criteria);
        assert_eq!(sub_score(&criterion, &missing, &value), 0.0);
    }

    #[test]
    fn enum_map_scores_work_mode() {
        let criterion = SoftCriterion {
            fact: "work_mode".into(),
            weight: 1.0,
            target: None,
            map: Some([("remote".to_string(), 1.0), ("hybrid".to_string(), 0.6)].into()),
            missing_score: None,
        };
        let fact = FactRef::parse("work_mode").unwrap();
        let value = resolve(&fact, &sample_facts(), &default_criteria());
        assert_eq!(sub_score(&criterion, &fact, &value), 1.0);
    }

    #[test]
    fn soft_criterion_requirements_enforced() {
        let bare_salary = SoftCriterion {
            fact: "salary_max_gbp".into(),
            weight: 1.0,
            target: None,
            map: None,
            missing_score: None,
        };
        assert!(FactRef::parse("salary_max_gbp").unwrap().check_soft(&bare_salary).is_err());

        let bare_enum = SoftCriterion {
            fact: "work_mode".into(),
            weight: 1.0,
            target: None,
            map: None,
            missing_score: None,
        };
        assert!(FactRef::parse("work_mode").unwrap().check_soft(&bare_enum).is_err());
    }
}
