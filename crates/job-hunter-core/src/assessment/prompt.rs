//! Prompt assembly. One LLM call per job performs both field extraction and
//! fit assessment against the candidate's experience (token efficiency:
//! single pass, bounded job text, bounded feedback).
//!
//! Prompts carry only what changes per call: a task pointer, the JSON
//! contract, and the data. The instruction prose lives in the agent
//! workspace (see `crate::workspace`), which the agent runs inside.

use crate::profile::Profile;
use crate::workspace::AgentTask;

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
                       "cv_evidence": "strong|moderate|weak|none (evidence in the experience document)",
                       "relevance": "number 0..1 (importance of the skill to the role)"}],
    "skills_missing": ["skills the role wants that the experience lacks"],
    "seniority_fit": "under|match|over|unspecified",
    "domain_familiarity": "number 0..1 (the candidate's familiarity with the role's industry)",
    "overall_cv_match": "number 0..1 (holistic candidate-to-role match)",
    "red_flags": ["string"], "green_flags": ["string"],
    "suggestions": ["actionable, candidate-specific advice for applying to this role"],
    "notes": "one short paragraph of nuance"
  },
  "confidence": {"salary": "number 0..1", "work_mode": "number 0..1", "overall": "number 0..1"}
}"#;

pub fn build_assessment_prompt(
    profile: &Profile,
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
    // The profile owns its own headings - see `Profile::sections`.
    let profile_sections = profile.sections();
    let pointer = AgentTask::Assess.instructions_pointer();
    format!(
        "Task: assess the job posting below against the candidate's experience.\n\
         {pointer}\n\
         Return ONLY a single JSON object matching this contract - no prose, no markdown fences:\n\
         {FACTS_CONTRACT}\n\
         {feedback_section}\n\
         {profile_sections}\n\
         ## Job posting\n{job}\n"
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

    fn profile(experience: &str, preferences: Option<&str>) -> Profile {
        Profile {
            experience: experience.into(),
            experience_hash: crate::sha256_hex(experience.as_bytes()),
            preferences: preferences.map(Into::into),
        }
    }

    #[test]
    fn prompt_contains_pointer_contract_and_data() {
        let p = build_assessment_prompt(
            &profile("MY EXPERIENCE", None),
            Some("- avoid agencies"),
            "JOB TEXT",
            100,
        );
        // Both forms of the pointer: the name a skill-aware agent resolves,
        // and the path any agent with a read tool can follow.
        assert!(p.contains("`assess` skill"));
        assert!(p.contains(".claude/skills/assess/SKILL.md"));
        assert!(p.contains("AGENTS.md"));
        assert!(p.contains("MY EXPERIENCE"));
        assert!(p.contains("- avoid agencies"));
        assert!(p.contains("JOB TEXT"));
        assert!(p.contains("schema_version"));
    }

    #[test]
    fn instruction_prose_lives_in_the_workspace_not_the_prompt() {
        // The hard rules ship in the assess skill (pinned by workspace.rs
        // tests); the prompt itself carries only the pointer.
        let p = build_assessment_prompt(&profile("EXP", None), None, "JOB", 100);
        assert!(!p.contains("Hard rules"));
        assert!(!p.contains("never collapse a stated range"));
    }

    #[test]
    fn feedback_section_omitted_when_empty() {
        let p = build_assessment_prompt(&profile("EXP", None), None, "JOB", 100);
        assert!(!p.contains("past feedback"));
    }

    #[test]
    fn preferences_ride_along_after_the_experience() {
        let p = build_assessment_prompt(
            &profile("ran the payments rewrite", Some("remote only, no on-call")),
            None,
            "JOB",
            100,
        );
        assert!(p.contains("## Candidate experience"));
        assert!(p.contains("ran the payments rewrite"));
        assert!(p.contains("## What the candidate wants from a role"));
        assert!(p.contains("no on-call"));
        // Order matters: the profile is complete before the posting appears.
        assert!(p.find("no on-call") < p.find("## Job posting"));
    }

    #[test]
    fn no_preferences_means_no_empty_heading() {
        let p = build_assessment_prompt(&profile("MY EXPERIENCE", None), None, "JOB", 100);
        assert!(!p.contains("wants from a role"));
    }

    #[test]
    fn experience_is_never_truncated_however_long() {
        // The size guard at load time is the only limit on the profile; the
        // job text budget must not leak onto it.
        let long = "y".repeat(50_000);
        let p = build_assessment_prompt(&profile(&long, None), None, "JOB", 100);
        assert!(p.contains(&long));
    }

    #[test]
    fn truncation_marks_cut() {
        let t = truncate_chars(&"x".repeat(50), 10);
        assert!(t.starts_with("xxxxxxxxxx"));
        assert!(t.contains("truncated"));
        assert_eq!(truncate_chars("short", 10), "short");
    }
}
