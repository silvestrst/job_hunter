use super::{ensure_score, job_label, Ctx};
use anyhow::Result;
use comfy_table::{presets, Table};
use job_hunter_core::db::JobFilter;

/// Recompute layer-2 scores for every assessed job under the current criteria.
/// Pure Rust - no LLM calls, instant - this is the payoff of the two-layer
/// design: edit the YAML criteria, run this, get a fresh ranking.
pub fn run() -> Result<()> {
    let ctx = Ctx::load()?;
    let criteria_hash = ctx.config.criteria_hash();
    let jobs = ctx.db.list_jobs(&JobFilter::default())?;

    struct Row {
        label: String,
        score: f64,
        passed: bool,
        previous: Option<f64>,
    }
    let mut rows = Vec::new();
    let mut unassessed = 0usize;
    for job in &jobs {
        match ensure_score(&ctx, job.id, &criteria_hash)? {
            None => unassessed += 1,
            Some((assessment, score)) => {
                let previous = ctx.db.previous_score(assessment.id, &criteria_hash)?;
                rows.push((job.id, Row {
                    label: job_label(job),
                    score: score.score,
                    passed: score.hard_filter_passed,
                    previous: previous.map(|p| p.score),
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
    table.set_header(vec!["ID", "Score", "Δ prev criteria", "✓", "Job"]);
    for (id, row) in &rows {
        let delta = match row.previous {
            Some(p) => format!("{:+.1}", row.score - p),
            None => "-".to_string(),
        };
        table.add_row(vec![
            format!("{id}"),
            format!("{:.1}", row.score),
            delta,
            if row.passed { "✓".into() } else { "✗".to_string() },
            row.label.clone(),
        ]);
    }
    println!("{table}");
    if unassessed > 0 {
        println!("{unassessed} job(s) have no assessment yet - run `job-hunter assess`.");
    }
    Ok(())
}
