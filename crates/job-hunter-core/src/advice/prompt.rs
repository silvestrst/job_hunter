//! Prompt assembly for `cv-advice`. Unlike the assessment, this call is
//! deliberate and one-off per job, so it carries the CV *verbatim* - you can't
//! advise on restructuring a document you were only shown a digest of - plus
//! the supplements, the stored facts from the assessment, and the posting.
//!
//! As everywhere else, the prose lives in the agent workspace; this builds a
//! pointer, the JSON contract, and the data.

use crate::assessment::prompt::truncate_chars;
use crate::profile::ProfilePrompt;
use crate::workspace::AgentTask;

/// The JSON contract shown to the agent, kept beside the serde types in
/// `schema.rs` so the two are reviewed together.
const ADVICE_CONTRACT: &str = r#"{
  "schema_version": 1,
  "positioning": "1-2 sentences: how this candidate should frame themselves for THIS role",
  "rewrites": [{"section": "the CV section to change, named as the CV names it",
                "current": "string|null (that section's wording today, quoted verbatim)",
                "suggested": "the replacement text, ready to paste",
                "why": "string|null (what it buys with this posting)"}],
  "emphasise": ["CV material to lead with, expand, or move up"],
  "deemphasise": ["CV material to cut or shorten for this application"],
  "gaps": [{"gap": "a requirement the CV cannot evidence",
            "how_to_address": "string|null (honest way to bridge or preempt it)"}],
  "keywords": ["exact terms the posting uses that the CV does not"],
  "notes": "string|null (one short paragraph of nuance)"
}"#;

/// `facts_json` is the stored assessment for this job, passed through as-is:
/// it is already the distilled read of the posting, so the agent starts from
/// the same conclusions the score was built on rather than re-deriving them.
pub fn build_advice_prompt(
    profile: &ProfilePrompt,
    facts_json: &str,
    job_text: &str,
    max_job_chars: usize,
) -> String {
    let job = truncate_chars(job_text, max_job_chars);
    let profile_sections = profile.sections();
    let cv_text = &profile.cv_text;
    let pointer = AgentTask::CvAdvice.instructions_pointer();
    format!(
        "Task: advise how to restructure the CV below for the job posting below.\n\
         {pointer}\n\
         Return ONLY a single JSON object matching this contract - no prose, no markdown fences:\n\
         {ADVICE_CONTRACT}\n\n\
         ## Candidate CV (as it stands today)\n{cv_text}\n\
         {profile_sections}\n\
         ## Assessment of this posting (facts from the earlier assessment pass)\n{facts_json}\n\n\
         ## Job posting\n{job}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> ProfilePrompt {
        ProfilePrompt {
            cv_text: "MY CV".into(),
            background: Some("BACKGROUND DOC".into()),
            preferences: Some("WANTS DOC".into()),
        }
    }

    #[test]
    fn prompt_carries_pointer_contract_and_every_input() {
        let p = build_advice_prompt(&profile(), r#"{"fit":{"skills_missing":["kafka"]}}"#, "JOB TEXT", 100);
        assert!(p.contains("`cv-advice` skill"));
        assert!(p.contains(".claude/skills/cv-advice/SKILL.md"));
        assert!(p.contains("positioning"));
        assert!(p.contains("MY CV"));
        assert!(p.contains("BACKGROUND DOC"));
        assert!(p.contains("WANTS DOC"));
        assert!(p.contains("kafka"), "the stored assessment is an input");
        assert!(p.contains("JOB TEXT"));
    }

    #[test]
    fn cv_is_verbatim_while_the_posting_is_budgeted() {
        // You can't advise on restructuring a document you only saw a digest
        // of, so the CV is never trimmed here - only the posting is.
        let long_cv = "c".repeat(500);
        let p = build_advice_prompt(
            &ProfilePrompt { cv_text: long_cv.clone(), background: None, preferences: None },
            "{}",
            &"j".repeat(500),
            10,
        );
        assert!(p.contains(&long_cv));
        assert!(!p.contains(&"j".repeat(11)));
        assert!(p.contains("truncated"));
    }

    #[test]
    fn supplement_sections_are_omitted_when_unconfigured() {
        let p = build_advice_prompt(
            &ProfilePrompt { cv_text: "CV".into(), background: None, preferences: None },
            "{}",
            "JOB",
            100,
        );
        assert!(!p.contains("Additional background"));
        assert!(!p.contains("wants from a role"));
    }
}
