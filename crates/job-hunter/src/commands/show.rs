use super::{ensure_score, format_pct, format_salary, job_label, Ctx};
use anyhow::Result;
use comfy_table::{presets, Table};
use job_hunter_core::assessment::schema::PreferenceMatch;
use job_hunter_core::scoring::FactValue;

/// One item under a `→ `, with any further lines aligned beneath it. Items are
/// meant to be one line each, but nothing enforces that on the way in.
fn print_bulleted(text: &str) {
    let mut lines = text.trim_end().lines();
    let Some(first) = lines.next() else { return };
    println!("    → {first}");
    for line in lines {
        print_wrapped("      ", line);
    }
}

/// Free-form prose at a fixed indent, blank lines kept so paragraphs stay
/// paragraphs.
fn print_wrapped(indent: &str, text: &str) {
    for line in text.trim_end().lines() {
        if line.trim().is_empty() {
            println!();
        } else {
            println!("{indent}{line}");
        }
    }
}

fn fact_value_text(v: &FactValue) -> String {
    match v {
        FactValue::Number(n) => {
            if *n >= 1000.0 {
                format!("{n:.0}")
            } else {
                format!("{n:.2}")
            }
        }
        FactValue::Text(t) => t.clone(),
        FactValue::Missing => "missing".to_string(),
    }
}

pub fn run(id: i64) -> Result<()> {
    let ctx = Ctx::load()?;
    let job = ctx.db.get_job(id)?;
    let criteria_hash = ctx.config.criteria_hash();

    println!("# {} [{}]", job_label(&job), job.status);
    println!("  id:      {}", job.id);
    println!("  url:     {}", job.url);
    println!("  domain:  {}  source: {}  saved: {}", job.domain, job.source, job.created_at);
    if let Some(location) = &job.location {
        println!("  location: {location}");
    }
    if let Some(dup) = job.duplicate_of {
        println!("  duplicate of job #{dup} (same title + company)");
    }
    let duplicates = ctx.db.duplicates_of(id)?;
    if !duplicates.is_empty() {
        println!("  also seen at:");
        for d in &duplicates {
            println!("    - {} (job #{})", d.url, d.id);
        }
    }

    println!("\n## Original posting\n{}", job.content_text);

    match ensure_score(&ctx, id, &criteria_hash)? {
        None => println!("\nNot assessed yet - run `job-hunter assess {id}`."),
        Some((assessment, score)) => {
            let facts = assessment.facts()?;
            let instr = assessment
                .instructions_hash
                .as_deref()
                .map(|h| format!(" / instr {h}"))
                .unwrap_or_default();
            println!("\n## Assessment (experience {} / prompt v{}{instr}, {})",
                &assessment.experience_hash[..8.min(assessment.experience_hash.len())],
                assessment.prompt_version,
                assessment.created_at
            );
            if let Some(summary) = &facts.extracted.summary {
                println!("  {summary}");
            }
            println!("  salary: {}   mode: {}   type: {}   seniority: {}",
                format_salary(&facts),
                facts.extracted.work_mode.as_str(),
                facts.extracted.employment_type.as_str(),
                facts.extracted.seniority_level.as_str(),
            );
            if !facts.extracted.tech_stack.is_empty() {
                println!("  stack: {}", facts.extracted.tech_stack.join(", "));
            }
            if !facts.fit.skill_matches.is_empty() {
                let skills: Vec<String> = facts
                    .fit
                    .skill_matches
                    .iter()
                    .map(|m| format!("{} ({})", m.skill, m.cv_evidence.as_str()))
                    .collect();
                println!("  skills vs CV: {}", skills.join(", "));
            }
            if !facts.fit.skills_missing.is_empty() {
                println!("  missing: {}", facts.fit.skills_missing.join(", "));
            }
            for flag in &facts.fit.red_flags {
                println!("  ⚠ {flag}");
            }
            for flag in &facts.fit.green_flags {
                println!("  ✦ {flag}");
            }
            if let Some(notes) = &facts.fit.notes {
                println!("  notes: {notes}");
            }
            let suggestions = &facts.fit.suggestions;
            if !suggestions.is_empty() {
                println!("\n## How to approach this one");
                if let Some(advice) = &suggestions.advice {
                    print_wrapped("  ", advice);
                }
                for (heading, items) in
                    [("gaps", &suggestions.gaps), ("strengths", &suggestions.strengths)]
                {
                    if !items.is_empty() {
                        println!("\n  {heading}:");
                        for item in items {
                            print_bulleted(item);
                        }
                    }
                }
            }

            // Preferences are advisory: reported in full here, never scored.
            if facts.fit.preference_match != PreferenceMatch::Unspecified
                || facts.fit.preference_summary.is_some()
            {
                println!("\n## Preference match: {}", facts.fit.preference_match.as_str());
                if let Some(summary) = &facts.fit.preference_summary {
                    println!("  {summary}");
                }
                println!("  (advisory - what you want does not move the score)");
            }

            let breakdown = score.breakdown()?;
            println!("\n## Score: {}/100  hard filters: {}",
                format_pct(Some(score.score)),
                if score.hard_filter_passed { "passed ✓" } else { "FAILED ✗" }
            );
            println!("  the mean of the two fit measures, and nothing else:");
            println!("    CV match            {}", format_pct(breakdown.cv_match));
            println!("    domain familiarity  {}", format_pct(breakdown.domain_familiarity));
            if !breakdown.filters.is_empty() {
                let mut table = Table::new();
                table.load_style(presets::UTF8_BORDERS_ONLY);
                table.set_header(vec!["Hard filter", "Requirement", "Actual", "Outcome"]);
                for f in &breakdown.filters {
                    table.add_row(vec![
                        f.fact.clone(),
                        format!("{:?} {}", f.op, f.value),
                        fact_value_text(&f.actual),
                        format!("{:?}", f.outcome),
                    ]);
                }
                println!("{table}");
            }
        }
    }

    let events = ctx.db.status_events(id)?;
    if !events.is_empty() {
        println!("\n## History");
        for e in &events {
            let note = e.note.as_deref().map(|n| format!(" - {n}")).unwrap_or_default();
            println!("  {}  {} → {}{}", e.created_at, e.from_status.as_deref().unwrap_or("-"), e.to_status, note);
        }
    }

    let feedback = ctx.db.feedback_for_job(id)?;
    if !feedback.is_empty() {
        println!("\n## Feedback");
        for f in &feedback {
            println!("  {}  {} [{}]", f.created_at, f.text, f.state);
        }
    }
    Ok(())
}
