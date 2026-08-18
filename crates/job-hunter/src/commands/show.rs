use super::{ensure_score, format_salary, job_label, Ctx};
use anyhow::Result;
use comfy_table::{presets, Table};
use job_hunter_core::scoring::{FactValue, ScoreResult};

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

    match ensure_score(&ctx, id, &criteria_hash)? {
        None => println!("\nNot assessed yet - run `job-hunter assess {id}`."),
        Some((assessment, score)) => {
            let facts = assessment.facts()?;
            println!("\n## Assessment (cv {} / prompt v{}, {})",
                &assessment.cv_hash[..8.min(assessment.cv_hash.len())],
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
            if !facts.fit.suggestions.is_empty() {
                println!("  suggestions:");
                for s in &facts.fit.suggestions {
                    println!("    → {s}");
                }
            }

            println!("\n## Score: {:.1}/100  hard filters: {}",
                score.score,
                if score.hard_filter_passed { "passed ✓" } else { "FAILED ✗" }
            );
            let breakdown: ScoreResult = serde_json::from_str(&score.breakdown_json)?;
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
            if !breakdown.soft.is_empty() {
                let mut table = Table::new();
                table.load_style(presets::UTF8_BORDERS_ONLY);
                table.set_header(vec!["Criterion", "Weight", "Value", "Sub-score", "Points"]);
                for s in &breakdown.soft {
                    table.add_row(vec![
                        s.fact.clone(),
                        format!("{}", s.weight),
                        fact_value_text(&s.value),
                        format!("{:.2}", s.sub_score),
                        format!("{:.1}", s.contribution),
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
