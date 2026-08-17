//! Feedback loop: user corrections stored in SQLite, injected into future
//! assessment prompts as a compact digest, and periodically compacted by the
//! agent into distilled guidance rules.

use crate::db::{Db, FeedbackItem};
use anyhow::Result;

const MAX_ITEM_CHARS: usize = 300;

fn format_item(item: &FeedbackItem) -> String {
    let text = crate::assessment::prompt::truncate_chars(item.text.trim(), MAX_ITEM_CHARS);
    match &item.job_label {
        Some(label) => format!("- [job: {label}] {text}"),
        None => format!("- {text}"),
    }
}

/// The digest injected into assessment prompts: distilled guidance (if any)
/// plus the N most recent active feedback items. None when there is nothing.
pub fn build_prompt_digest(db: &Db, recent_n: usize) -> Result<Option<String>> {
    let mut sections = Vec::new();
    if let Some(digest) = db.get_feedback_digest()? {
        if !digest.trim().is_empty() {
            sections.push(digest.trim().to_string());
        }
    }
    let items = db.active_feedback(Some(recent_n))?;
    if !items.is_empty() {
        let lines: Vec<String> = items.iter().map(format_item).collect();
        sections.push(lines.join("\n"));
    }
    if sections.is_empty() {
        return Ok(None);
    }
    Ok(Some(sections.join("\n")))
}

/// Prompt for `feedback --compact`: distill all active items (plus any prior
/// digest) into a small durable rule set.
pub fn build_compaction_prompt(existing_digest: Option<&str>, items: &[FeedbackItem]) -> String {
    let existing = match existing_digest {
        Some(d) if !d.trim().is_empty() => {
            format!("## Existing distilled guidance\n{}\n\n", d.trim())
        }
        _ => String::new(),
    };
    let lines: Vec<String> = items.iter().map(format_item).collect();
    format!(
        "You maintain assessment guidance for a job-search assistant. The candidate has \
         given feedback on past job assessments. Distill the existing guidance and the new \
         feedback below into AT MOST 10 durable guidance rules, 200 words total maximum. \
         Keep only lessons that transfer to future assessments; drop one-off remarks. \
         Return only the rules as a markdown bullet list, no preamble.\n\n\
         {existing}## New feedback\n{}\n",
        lines.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_combines_distilled_and_recent() {
        let db = Db::open_in_memory().unwrap();
        assert!(build_prompt_digest(&db, 10).unwrap().is_none());

        db.set_feedback_digest("- distrust vague salary claims").unwrap();
        db.insert_feedback(None, "I prefer product companies over agencies").unwrap();
        let digest = build_prompt_digest(&db, 10).unwrap().unwrap();
        assert!(digest.contains("distrust vague salary claims"));
        assert!(digest.contains("product companies"));
    }

    #[test]
    fn recent_n_caps_items() {
        let db = Db::open_in_memory().unwrap();
        for i in 0..5 {
            db.insert_feedback(None, &format!("item {i}")).unwrap();
        }
        let digest = build_prompt_digest(&db, 2).unwrap().unwrap();
        assert!(digest.contains("item 4"));
        assert!(digest.contains("item 3"));
        assert!(!digest.contains("item 2"));
    }

    #[test]
    fn compaction_prompt_includes_all() {
        let db = Db::open_in_memory().unwrap();
        db.insert_feedback(None, "too many contract roles scored high").unwrap();
        let items = db.active_feedback(None).unwrap();
        let prompt = build_compaction_prompt(Some("- old rule"), &items);
        assert!(prompt.contains("old rule"));
        assert!(prompt.contains("contract roles"));
        assert!(prompt.contains("AT MOST 10"));
    }
}
