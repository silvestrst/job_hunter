//! Prompt assembly. One LLM call per job performs both field extraction and
//! CV-relative fit assessment (token efficiency: single pass, compact CV,
//! bounded job text, bounded feedback).

/// The JSON contract shown to the agent. Kept as one literal so the schema the
/// model sees and the serde types in `schema.rs` are reviewed side by side.
const FACTS_CONTRACT: &str = r#"{
  "schema_version": 2,
  "extracted": {
    "job_title": "string|null", "company": "string|null", "location": "string|null",
    "country": "ISO 3166-1 alpha-2 string|null",
    "work_mode": "remote|hybrid|onsite|unspecified",
    "hybrid_days_in_office": "integer|null",
    "remote_restriction": "string|null",
    "salary": {"min": "number|null", "max": "number|null",
               "currency": "ISO 4217 string|null",
               "period": "year|day|hour|unspecified",
               "is_estimate": "boolean (true if the posting didn't state a salary and you inferred this range)",
               "raw_text": "string|null (verbatim as posted; null if inferred)"},
    "employment_type": "permanent|contract|freelance|internship|unspecified",
    "seniority_level": "junior|mid|senior|staff|principal|lead|manager|unspecified",
    "visa_sponsorship": "yes|no|unspecified",
    "tech_stack": ["string"],
    "industry": "string|null",
    "posted_date": "ISO date string|null",
    "summary": "2-3 neutral sentences describing the role"
  },
  "fit": {
    "skill_matches": [{"skill": "string", "required": "boolean",
                       "years_required": "number|null",
                       "cv_evidence": "strong|moderate|weak|none",
                       "relevance": "number 0..1 (importance of the skill to the role)"}],
    "skills_missing": ["skills the role wants that the CV lacks"],
    "seniority_fit": "under|match|over|unspecified",
    "domain_familiarity": "number 0..1 (CV's familiarity with the role's industry)",
    "overall_cv_match": "number 0..1 (holistic CV-to-role match)",
    "red_flags": ["string"], "green_flags": ["string"],
    "suggestions": ["actionable, CV-specific advice for applying to this role"],
    "notes": "one short paragraph of nuance"
  },
  "confidence": {"salary": "number 0..1", "work_mode": "number 0..1", "overall": "number 0..1"}
}"#;

pub fn build_assessment_prompt(
    cv_text: &str,
    feedback_digest: Option<&str>,
    job_text: &str,
    max_job_chars: usize,
) -> String {
    let job = truncate_chars(job_text, max_job_chars);
    let feedback_section = match feedback_digest {
        Some(d) if !d.trim().is_empty() => format!(
            "\n## Assessment guidance from the candidate's past feedback\n{d}\n"
        ),
        _ => String::new(),
    };
    format!(
        "You assess job postings against a candidate's CV.\n\
         Return ONLY a single JSON object matching this contract - no prose, no markdown fences:\n\
         {FACTS_CONTRACT}\n\n\
         Hard rules:\n\
         - Extract facts only; never judge against preferences (no \"salary is good\").\n\
         - Numbers as numbers, not prose. Salary in the posting's own currency and period.\n\
         - If the posting states a salary range, set salary.min and salary.max to the range's \
         low and high ends - never collapse a stated range into one number. A single stated \
         figure goes in both min and max.\n\
         - Unknown or unstated -> null or \"unspecified\". Never guess - EXCEPT salary: if the \
         posting doesn't state one, estimate a realistic market range from the job title, \
         seniority, location, industry, and tech stack; set salary.is_estimate=true, \
         salary.raw_text=null, and lower confidence.salary to reflect the guess. If you can't \
         even guess, leave salary.min/max null.\n\
         - fit.* measures the CV below against this posting.\n\
         - List every skill the posting emphasises in fit.skill_matches, matched or not.\n\
         - fit.suggestions: 2-4 concrete, actionable suggestions for THIS candidate applying to \
         THIS posting - what CV experience to foreground, gaps to address or preempt, questions \
         worth asking. Specific to this CV and posting, not generic career advice.\n\
         {feedback_section}\
         ## Candidate CV\n{cv_text}\n\n\
         ## Job posting\n{job}\n"
    )
}

pub fn build_cv_digest_prompt(cv_text: &str, max_words: usize) -> String {
    format!(
        "Distill the CV below into at most {max_words} words for use as assessment context.\n\
         Preserve EVERY skill, technology, employer, role title, duration, domain, and \
         qualification - compress prose and formatting only; do not summarize away detail.\n\
         Return only the distilled text, no preamble.\n\n{cv_text}"
    )
}

/// Truncate on a char boundary, appending a marker so the model knows text was cut.
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars).collect();
    format!("{cut}\n[... truncated ...]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_contains_all_sections() {
        let p = build_assessment_prompt("MY CV", Some("- avoid agencies"), "JOB TEXT", 100);
        assert!(p.contains("MY CV"));
        assert!(p.contains("- avoid agencies"));
        assert!(p.contains("JOB TEXT"));
        assert!(p.contains("schema_version"));
    }

    #[test]
    fn instructs_salary_estimation_when_unstated() {
        let p = build_assessment_prompt("CV", None, "JOB", 100);
        assert!(p.contains("estimate a realistic market range"));
        assert!(p.contains("is_estimate=true"));
    }

    #[test]
    fn instructs_preserving_stated_salary_ranges() {
        let p = build_assessment_prompt("CV", None, "JOB", 100);
        assert!(p.contains("never collapse a stated range into one number"));
    }

    #[test]
    fn instructs_actionable_suggestions() {
        let p = build_assessment_prompt("CV", None, "JOB", 100);
        assert!(p.contains("fit.suggestions"));
        assert!(p.contains("\"suggestions\""));
    }

    #[test]
    fn feedback_section_omitted_when_empty() {
        let p = build_assessment_prompt("CV", None, "JOB", 100);
        assert!(!p.contains("past feedback"));
    }

    #[test]
    fn truncation_marks_cut() {
        let t = truncate_chars(&"x".repeat(50), 10);
        assert!(t.starts_with("xxxxxxxxxx"));
        assert!(t.contains("truncated"));
        assert_eq!(truncate_chars("short", 10), "short");
    }
}
