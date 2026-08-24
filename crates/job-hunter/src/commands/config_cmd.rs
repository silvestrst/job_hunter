use anyhow::{bail, Context, Result};
use job_hunter_core::config::{Config, DEFAULT_CONFIG_YAML};
use job_hunter_core::paths::{expand_tilde, Paths};
use job_hunter_core::token;
use job_hunter_core::workspace::{AgentTask, AgentWorkspace, FileStatus};
use std::path::PathBuf;

pub fn init(force: bool) -> Result<()> {
    let paths = Paths::resolve()?;
    if paths.config_file.exists() && !force {
        bail!(
            "config already exists at {} (use --force to overwrite)",
            paths.config_file.display()
        );
    }
    if let Some(parent) = paths.config_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&paths.config_file, DEFAULT_CONFIG_YAML)
        .with_context(|| format!("cannot write {}", paths.config_file.display()))?;
    println!("Wrote {}", paths.config_file.display());

    // Agent workspace: missing and outdated instruction files; edits are never
    // touched.
    let (workspace, written) = AgentWorkspace::ensure(paths.agent_workspace_dir.clone())?;
    for (rel, _) in workspace.file_status() {
        if written.contains(&rel) {
            println!("Wrote {}", workspace.dir.join(rel).display());
        } else {
            println!("Kept  {} (your edits)", workspace.dir.join(rel).display());
        }
    }

    println!("\nNext steps:");
    println!("  1. Edit resources.experience_path to point at your experience document.");
    println!("  2. Adjust criteria.hard_filters to rule out what you won't consider.");
    println!("  3. Review the agent instructions in {}.", workspace.dir.display());
    println!("  4. `job-hunter config check` to validate.");
    println!("  5. `job-hunter serve` and load the browser extension.");
    Ok(())
}

pub fn check() -> Result<()> {
    let paths = Paths::resolve()?;
    let config = Config::load(&paths.config_file)?;

    println!("config:   {}", paths.config_file.display());
    println!("database: {}", paths.db_file.display());

    // Every configured input file, checked the way `assess` will check it:
    // size on disk against the guard, before anything is opened. A typo'd
    // path is worth catching here rather than at the next `assess`.
    let limit = config.resources.max_file_bytes;
    let mut inputs: Vec<(&str, PathBuf)> =
        vec![("experience:", expand_tilde(&config.resources.experience_path))];
    if let Some(path) = &config.resources.preferences_path {
        inputs.push(("wants:", expand_tilde(path)));
    }
    for (label, path) in inputs {
        match std::fs::metadata(&path) {
            Ok(meta) if !meta.is_file() => {
                println!("{label:<11} {} - NOT A FILE", path.display())
            }
            Ok(meta) if meta.len() > limit => println!(
                "{label:<11} {} - TOO BIG: {} bytes, over the {limit}-byte limit",
                path.display(),
                meta.len()
            ),
            Ok(meta) => {
                println!("{label:<11} {} ({} of {limit} bytes)", path.display(), meta.len())
            }
            Err(e) => println!("{label:<11} {} - NOT READABLE: {e}", path.display()),
        }
    }

    let resolved_command: Vec<String> = config
        .agent
        .command
        .iter()
        .map(|arg| match &config.agent.model {
            Some(model) => arg.replace("{model}", model),
            None => arg.clone(),
        })
        .collect();
    println!("agent:    {} (output: {:?})", resolved_command.join(" "), config.agent.output);
    println!("model:    {}", config.agent.model.as_deref().unwrap_or("(none - using agent CLI's own default)"));

    // Workspace inspection is read-only: missing files are reported, not written.
    let workspace = AgentWorkspace::open(AgentWorkspace::resolve_dir(&config.agent, &paths));
    println!("workspace: {} (agent cwd)", workspace.dir.display());
    let statuses = workspace.file_status();
    for (rel, status) in &statuses {
        let label = match status {
            FileStatus::Default => "default",
            FileStatus::Outdated => "outdated default (upgraded on next use or `config init`)",
            FileStatus::Edited => "edited",
            FileStatus::Missing => "missing (created on next use or `config init`)",
        };
        println!("  {rel:<42} {label}");
    }
    if statuses.iter().any(|(_, s)| *s == FileStatus::Edited) {
        println!("  (edited files are yours - delete one to restore the shipped default)");
    }
    if statuses.iter().all(|(_, s)| *s != FileStatus::Missing) {
        println!(
            "  assess instructions hash: {} (a change re-runs affected assessments)",
            workspace.instructions_hash(AgentTask::Assess)?
        );
    }
    println!("server:   http://127.0.0.1:{}", config.server.port);
    match token::load(&paths.api_token_file)? {
        Some(t) => println!("token:    {t}"),
        None => println!("token:    (generated on first `job-hunter serve`)"),
    }
    println!(
        "criteria: {} hard filter(s) - hash {} (filtering only; the score is the mean of \
         overall_cv_match and domain_familiarity)",
        config.criteria.hard_filters.len(),
        config.criteria_hash()
    );
    println!("\nConfig is valid.");
    Ok(())
}
