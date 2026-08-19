mod commands;
mod server;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "job-hunter", version, about = "Personal job-search tool: save postings from the browser, assess them with an LLM agent against your CV, rank them with configurable criteria.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List saved jobs ranked by score (default: not-yet-applied, filters passed)
    Jobs(commands::jobs::JobsArgs),
    /// Show everything about one job: fields, facts, score breakdown, history
    Show { id: i64 },
    /// Change a job's status (applied, progressing, rejected, dismissed, new)
    Status {
        id: i64,
        status: String,
        #[arg(long)]
        note: Option<String>,
    },
    /// Run the LLM assessment for jobs that don't have one yet
    Assess(commands::assess::AssessArgs),
    /// Suggest how to restructure your CV for specific jobs (LLM; needs an assessment)
    CvAdvice(commands::cv_advice::CvAdviceArgs),
    /// Recompute all layer-2 scores for the current criteria (no LLM, instant)
    Rescore,
    /// Record feedback on assessments, list it, or compact it into guidance
    Feedback(commands::feedback_cmd::FeedbackArgs),
    /// Run the local ingest server the browser extension talks to
    Serve {
        #[arg(long)]
        port: Option<u16>,
    },
    /// Install a systemd user service so the server is always running
    InstallService {
        /// Also run systemctl daemon-reload + enable --now
        #[arg(long)]
        now: bool,
    },
    /// Manage the configuration file
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Delete a mis-saved job (cascades to its assessments and scores)
    Delete { id: i64 },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Write a commented default config to ~/.config/job-hunter/config.yaml
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Validate the config and print resolved paths and the criteria hash
    Check,
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Jobs(args) => commands::jobs::run(args),
        Command::Show { id } => commands::show::run(id),
        Command::Status { id, status, note } => commands::status_cmd::run(id, &status, note.as_deref()),
        Command::Assess(args) => commands::assess::run(args),
        Command::CvAdvice(args) => commands::cv_advice::run(args),
        Command::Rescore => commands::rescore::run(),
        Command::Feedback(args) => commands::feedback_cmd::run(args),
        Command::Serve { port } => commands::serve_cmd::run(port),
        Command::InstallService { now } => commands::service::run(now),
        Command::Config { action } => match action {
            ConfigAction::Init { force } => commands::config_cmd::init(force),
            ConfigAction::Check => commands::config_cmd::check(),
        },
        Command::Delete { id } => commands::delete::run(id),
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
