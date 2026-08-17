use super::Ctx;
use anyhow::{bail, Result};
use clap::Args;
use job_hunter_core::agent::AgentRunner;
use job_hunter_core::feedback;

#[derive(Args)]
pub struct FeedbackArgs {
    /// Job id the feedback refers to
    pub id: Option<i64>,
    /// The feedback text, e.g. "scored too high - this is really a contract role"
    pub text: Option<String>,
    /// Record feedback not tied to one job
    #[arg(long, value_name = "TEXT")]
    pub general: Option<String>,
    /// List active feedback
    #[arg(long)]
    pub list: bool,
    /// Distill all active feedback into guidance rules via the agent (spends tokens)
    #[arg(long)]
    pub compact: bool,
}

const COMPACT_HINT_THRESHOLD: usize = 20;

pub fn run(args: FeedbackArgs) -> Result<()> {
    let ctx = Ctx::load()?;

    if args.list {
        let items = ctx.db.active_feedback(None)?;
        if let Some(digest) = ctx.db.get_feedback_digest()? {
            println!("## Distilled guidance\n{digest}\n");
        }
        if items.is_empty() {
            println!("No active feedback.");
        } else {
            println!("## Active feedback ({})", items.len());
            for item in items.iter().rev() {
                let scope = item.job_label.clone().unwrap_or_else(|| "general".into());
                println!("  #{} [{}] {} ({})", item.id, scope, item.text, item.created_at);
            }
        }
        return Ok(());
    }

    if args.compact {
        let items = ctx.db.active_feedback(None)?;
        if items.is_empty() {
            println!("No active feedback to compact.");
            return Ok(());
        }
        let existing = ctx.db.get_feedback_digest()?;
        let prompt = feedback::build_compaction_prompt(existing.as_deref(), &items);
        println!("Distilling {} feedback item(s) via the agent...", items.len());
        let digest = AgentRunner::new(&ctx.config.agent).run_text(&prompt)?;
        if digest.trim().is_empty() {
            bail!("agent returned an empty digest - feedback left untouched");
        }
        ctx.db.set_feedback_digest(digest.trim())?;
        let n = ctx.db.mark_all_feedback_compacted()?;
        println!("Compacted {n} item(s) into:\n\n{}", digest.trim());
        return Ok(());
    }

    if let Some(text) = &args.general {
        if args.id.is_some() || args.text.is_some() {
            bail!("--general takes its own text; don't combine it with a job id");
        }
        ctx.db.insert_feedback(None, text)?;
        println!("Recorded general feedback. It will inform future assessments.");
    } else {
        let (Some(id), Some(text)) = (args.id, &args.text) else {
            bail!(
                "usage: job-hunter feedback <job-id> \"text\"  (or --general \"text\", --list, --compact)"
            );
        };
        ctx.db.insert_feedback(Some(id), text)?;
        println!("Recorded feedback on job #{id}. It will inform future assessments.");
        println!("(Existing assessments are not re-run - use `job-hunter assess {id} --force` to apply it retroactively.)");
    }

    let active = ctx.db.active_feedback(None)?.len();
    if active > COMPACT_HINT_THRESHOLD {
        println!("{active} active feedback items - consider `job-hunter feedback --compact` to distill them.");
    }
    Ok(())
}
