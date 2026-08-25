use super::{job_label, Ctx};
use anyhow::{bail, Result};
use clap::Args;
use std::io::{self, Write};

#[derive(Args)]
pub struct DeleteArgs {
    /// Job id to delete (cascades to its assessments and scores)
    pub id: Option<i64>,
    /// Delete every job instead of one (cascades to all assessments, scores,
    /// and status history; feedback and its distilled digest are kept)
    #[arg(long, conflicts_with = "id")]
    pub all: bool,
    /// Skip the confirmation prompt for --all
    #[arg(long, requires = "all")]
    pub yes: bool,
}

pub fn run(args: DeleteArgs) -> Result<()> {
    let ctx = Ctx::load()?;
    if args.all {
        return delete_all(&ctx, args.yes);
    }
    let Some(id) = args.id else {
        bail!("usage: job-hunter delete <id>  (or --all)");
    };
    let job = ctx.db.get_job(id)?;
    ctx.db.delete_job(id)?;
    println!("Deleted job #{id} ({}) and its assessments/scores.", job_label(&job));
    Ok(())
}

fn delete_all(ctx: &Ctx, yes: bool) -> Result<()> {
    let n = ctx.db.count_jobs()?;
    if n == 0 {
        println!("No jobs to delete.");
        return Ok(());
    }
    if !yes {
        print!("Delete all {n} job(s) and their assessments/scores/history? Feedback is kept. This cannot be undone. Type 'yes' to confirm: ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if input.trim() != "yes" {
            println!("Aborted.");
            return Ok(());
        }
    }
    let n = ctx.db.delete_all_jobs()?;
    println!("Deleted {n} job(s) and their assessments/scores/history. Feedback was kept.");
    Ok(())
}
