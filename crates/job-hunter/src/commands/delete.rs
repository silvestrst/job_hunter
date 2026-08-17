use super::{job_label, Ctx};
use anyhow::Result;

pub fn run(id: i64) -> Result<()> {
    let ctx = Ctx::load()?;
    let job = ctx.db.get_job(id)?;
    ctx.db.delete_job(id)?;
    println!("Deleted job #{id} ({}) and its assessments/scores.", job_label(&job));
    Ok(())
}
