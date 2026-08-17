use super::{ensure_score, format_age, format_salary, Ctx};
use anyhow::Result;
use clap::Args;
use comfy_table::{presets, Cell, Table};
use job_hunter_core::db::{JobFilter, JobStatus};

#[derive(Args)]
pub struct JobsArgs {
    /// Include every status
    #[arg(long, conflicts_with_all = ["applied", "rejected", "progressing", "status"])]
    pub all: bool,
    #[arg(long, conflicts_with_all = ["rejected", "progressing", "status"])]
    pub applied: bool,
    #[arg(long, conflicts_with_all = ["progressing", "status"])]
    pub rejected: bool,
    #[arg(long, conflicts_with = "status")]
    pub progressing: bool,
    /// Filter by an explicit status
    #[arg(long)]
    pub status: Option<String>,
    /// Also show jobs that failed the hard filters (marked ✗)
    #[arg(long)]
    pub include_filtered: bool,
    #[arg(long)]
    pub min_score: Option<f64>,
    #[arg(long)]
    pub limit: Option<usize>,
}

struct Row {
    id: i64,
    score: Option<f64>,
    filters_passed: Option<bool>,
    title: String,
    company: String,
    salary: String,
    mode: String,
    status: String,
    age: String,
}

pub fn run(args: JobsArgs) -> Result<()> {
    let ctx = Ctx::load()?;
    let criteria_hash = ctx.config.criteria_hash();

    let statuses = if args.all {
        None
    } else if args.applied {
        Some(vec![JobStatus::Applied])
    } else if args.rejected {
        Some(vec![JobStatus::Rejected])
    } else if args.progressing {
        Some(vec![JobStatus::Progressing])
    } else if let Some(s) = &args.status {
        Some(vec![JobStatus::parse(s)?])
    } else {
        Some(vec![JobStatus::New, JobStatus::Assessed])
    };
    let default_view = statuses
        .as_deref()
        .is_some_and(|s| s == [JobStatus::New, JobStatus::Assessed]);

    let jobs = ctx.db.list_jobs(&JobFilter { statuses, include_duplicates: false })?;
    let mut rows = Vec::new();
    let mut hidden_by_filters = 0usize;
    for job in &jobs {
        let scored = ensure_score(&ctx, job.id, &criteria_hash)?;
        let (score, filters_passed, salary, mode) = match &scored {
            Some((assessment, score)) => {
                let facts = assessment.facts()?;
                (
                    Some(score.score),
                    Some(score.hard_filter_passed),
                    format_salary(&facts),
                    facts.extracted.work_mode.as_str().to_string(),
                )
            }
            None => (None, None, "-".to_string(), "-".to_string()),
        };
        // The default view hides hard-filter failures; explicit status views show everything.
        if default_view && !args.include_filtered && filters_passed == Some(false) {
            hidden_by_filters += 1;
            continue;
        }
        if let Some(min) = args.min_score {
            if score.is_none() || score.unwrap() < min {
                continue;
            }
        }
        rows.push(Row {
            id: job.id,
            score,
            filters_passed,
            title: job.title.clone().unwrap_or_else(|| "(not assessed yet)".into()),
            company: job.company.clone().unwrap_or_else(|| "-".into()),
            salary,
            mode: if mode == "unspecified" { "-".into() } else { mode },
            status: job.status.clone(),
            age: format_age(&job.created_at),
        });
    }

    // Highest score first; unassessed jobs at the bottom, newest first.
    rows.sort_by(|a, b| match (a.score, b.score) {
        (Some(x), Some(y)) => y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.id.cmp(&a.id),
    });
    if let Some(limit) = args.limit {
        rows.truncate(limit);
    }

    if rows.is_empty() {
        println!("No jobs to show.");
    } else {
        let mut table = Table::new();
        table.load_style(presets::UTF8_BORDERS_ONLY);
        table.set_header(vec!["ID", "Score", "✓", "Title", "Company", "Salary", "Mode", "Status", "Age"]);
        for row in &rows {
            table.add_row(vec![
                Cell::new(row.id),
                Cell::new(row.score.map_or("-".into(), |s| format!("{s:.1}"))),
                Cell::new(match row.filters_passed {
                    Some(true) => "✓",
                    Some(false) => "✗",
                    None => "·",
                }),
                Cell::new(&row.title),
                Cell::new(&row.company),
                Cell::new(&row.salary),
                Cell::new(&row.mode),
                Cell::new(&row.status),
                Cell::new(&row.age),
            ]);
        }
        println!("{table}");
    }

    let unassessed = rows.iter().filter(|r| r.score.is_none()).count();
    if unassessed > 0 {
        println!("{unassessed} job(s) not assessed yet - run `job-hunter assess`.");
    }
    if hidden_by_filters > 0 {
        println!("{hidden_by_filters} job(s) hidden by hard filters - use --include-filtered to see them.");
    }
    Ok(())
}
