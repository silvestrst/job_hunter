use super::{job_label, Ctx};
use anyhow::Result;
use job_hunter_core::db::JobStatus;

pub fn run(id: i64, status: &str, note: Option<&str>) -> Result<()> {
    let ctx = Ctx::load()?;
    let to = JobStatus::parse(status)?;
    let job = ctx.db.set_status(id, to, note)?;
    println!("job #{} ({}) → {}", id, job_label(&job), job.status);
    Ok(())
}
