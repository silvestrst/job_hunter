use super::{ensure_score, format_pct, job_label, Ctx};
use anyhow::Result;
use comfy_table::{presets, Table};
use job_hunter_core::db::JobFilter;

/// Re-run the hard filters for every assessed job under the current criteria.
/// Pure Rust - no LLM calls, instant - this is the payoff of the two-layer
/// design: edit the YAML criteria, run this, get a fresh cut of the list.
///
/// The score itself cannot move here: it is the mean of two measures the agent
/// produced, and criteria no longer weight anything. So the column worth
/// showing is the filter verdict and whether the edit flipped it.
pub fn run() -> Result<()> {
    let ctx = Ctx::load()?;
    let criteria_hash = ctx.config.criteria_hash();
    let jobs = ctx.db.list_jobs(&JobFilter::default())?;

    struct Row {
        label: String,
        score: f64,
        passed: bool,
        was_passing: Option<bool>,
    }
    let mut rows = Vec::new();
    let mut unassessed = 0usize;
    let mut flipped = 0usize;
    for job in &jobs {
        match ensure_score(&ctx, job.id, &criteria_hash)? {
            None => unassessed += 1,
            Some((assessment, score)) => {
                let previous = ctx.db.previous_score(assessment.id, &criteria_hash)?;
                let was_passing = previous.map(|p| p.hard_filter_passed);
                if was_passing.is_some_and(|was| was != score.hard_filter_passed) {
                    flipped += 1;
                }
                rows.push((job.id, Row {
                    label: job_label(job),
                    score: score.score,
                    passed: score.hard_filter_passed,
                    was_passing,
                }));
            }
        }
    }
    rows.sort_by(|a, b| b.1.score.partial_cmp(&a.1.score).unwrap_or(std::cmp::Ordering::Equal));

    if rows.is_empty() {
        println!("No assessed jobs to score.");
        return Ok(());
    }
    println!("Criteria hash {criteria_hash}:");
    let mut table = Table::new();
    table.load_style(presets::UTF8_BORDERS_ONLY);
    table.set_header(vec!["ID", "Score", "Filters", "vs prev criteria", "Job"]);
    for (id, row) in &rows {
        let change = match row.was_passing {
            Some(was) if was == row.passed => "unchanged".to_string(),
            Some(true) => "now filtered out".to_string(),
            Some(false) => "now passing".to_string(),
            None => "-".to_string(),
        };
        table.add_row(vec![
            format!("{id}"),
            format_pct(Some(row.score)),
            if row.passed { "✓".into() } else { "✗".to_string() },
            change,
            row.label.clone(),
        ]);
    }
    println!("{table}");
    let passing = rows.iter().filter(|(_, r)| r.passed).count();
    println!(
        "{passing} of {} passing the filters, {flipped} changed since the previous criteria.",
        rows.len()
    );
    if unassessed > 0 {
        println!("{unassessed} job(s) have no assessment yet - run `job-hunter assess`.");
    }
    Ok(())
}
