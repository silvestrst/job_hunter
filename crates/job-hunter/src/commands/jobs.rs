use super::{ensure_score, format_age, format_pct, format_salary, Ctx};
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
    cv_match: Option<f64>,
    domain: Option<f64>,
    preference: String,
    filters_passed: Option<bool>,
    title: String,
    company: String,
    salary: String,
    mode: String,
    level: String,
    status: String,
    age: String,
}

/// "-" for the enum's absent case, so an unassessed preference never reads as
/// a verdict.
fn or_dash(value: &str) -> String {
    if value == "unspecified" {
        "-".to_string()
    } else {
        value.to_string()
    }
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
    let mut hidden = 0usize;
    let mut trimmed = 0usize;
    for job in &jobs {
        let scored = ensure_score(&ctx, job.id, &criteria_hash)?;
        let mut row = Row {
            id: job.id,
            score: None,
            cv_match: None,
            domain: None,
            preference: "-".into(),
            filters_passed: None,
            title: job.title.clone().unwrap_or_else(|| "(not assessed yet)".into()),
            company: job.company.clone().unwrap_or_else(|| "-".into()),
            salary: "-".into(),
            mode: "-".into(),
            level: "-".into(),
            status: job.status.clone(),
            age: format_age(&job.created_at),
        };
        if let Some((assessment, score)) = &scored {
            let facts = assessment.facts()?;
            let breakdown = score.breakdown()?;
            row.score = Some(score.score);
            row.cv_match = breakdown.cv_match;
            row.domain = breakdown.domain_familiarity;
            row.preference = or_dash(facts.fit.preference_match.as_str());
            row.filters_passed = Some(score.hard_filter_passed);
            row.salary = format_salary(&facts);
            row.mode = or_dash(facts.extracted.work_mode.as_str());
            row.level = or_dash(facts.extracted.seniority_level.as_str());
        }
        // Filter failures are counted in every view, and hidden only in the
        // default one: a job you have already applied to should not vanish
        // because the criteria moved on.
        if row.filters_passed == Some(false) && default_view && !args.include_filtered {
            hidden += 1;
            continue;
        }
        // Compared against the *displayed* score, so `--min-score 70` keeps
        // every row the table shows as 70 rather than dropping a 69.6.
        if let Some(min) = args.min_score {
            if row.score.is_none_or(|s| s.round() < min) {
                trimmed += 1;
                continue;
            }
        }
        rows.push(row);
    }

    // Highest score first; unassessed jobs at the bottom, newest first.
    rows.sort_by(|a, b| match (a.score, b.score) {
        (Some(x), Some(y)) => y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.id.cmp(&a.id),
    });
    if let Some(limit) = args.limit {
        trimmed += rows.len().saturating_sub(limit);
        rows.truncate(limit);
    }

    if rows.is_empty() {
        println!("No jobs to show.");
    } else {
        let mut table = Table::new();
        table.load_style(presets::UTF8_BORDERS_ONLY);
        table.set_header(vec![
            "ID", "Score", "CV", "Domain", "Pref", "✓", "Title", "Company", "Salary", "Mode",
            "Level", "Status", "Age",
        ]);
        for row in &rows {
            table.add_row(vec![
                Cell::new(row.id),
                Cell::new(format_pct(row.score)),
                Cell::new(format_pct(row.cv_match)),
                Cell::new(format_pct(row.domain)),
                Cell::new(&row.preference),
                Cell::new(match row.filters_passed {
                    Some(true) => "✓",
                    Some(false) => "✗",
                    None => "·",
                }),
                Cell::new(&row.title),
                Cell::new(&row.company),
                Cell::new(&row.salary),
                Cell::new(&row.mode),
                Cell::new(&row.level),
                Cell::new(&row.status),
                Cell::new(&row.age),
            ]);
        }
        println!("{table}");
    }

    // One summary line, always. Every count is about the rows on screen except
    // `filtered out` and `trimmed`, which are exactly the rows that are not -
    // so the parts add up to the jobs this view considered.
    let unassessed = rows.iter().filter(|r| r.score.is_none()).count();
    let failing = rows.iter().filter(|r| r.filters_passed == Some(false)).count();
    let mut parts = vec![format!("{} shown", rows.len())];
    if hidden > 0 {
        parts.push(format!("{hidden} filtered out"));
    }
    if failing > 0 {
        parts.push(format!("{failing} shown failing the filters ✗"));
    }
    if trimmed > 0 {
        parts.push(format!("{trimmed} trimmed"));
    }
    if unassessed > 0 {
        parts.push(format!("{unassessed} not assessed"));
    }
    println!("{}", parts.join(" · "));
    if hidden > 0 {
        println!("Use --include-filtered to see the filtered jobs.");
    }
    if unassessed > 0 {
        println!("Run `job-hunter assess` to assess the rest.");
    }
    Ok(())
}
