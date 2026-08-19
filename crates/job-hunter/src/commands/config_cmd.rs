use anyhow::{bail, Context, Result};
use job_hunter_core::config::{Config, DEFAULT_CONFIG_YAML};
use job_hunter_core::paths::{expand_tilde, Paths};
use job_hunter_core::token;
use job_hunter_core::workspace::{AgentTask, AgentWorkspace, FileStatus};

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

    // Agent workspace: missing instruction files only; edits are never touched.
    let (workspace, created) = AgentWorkspace::ensure(paths.agent_workspace_dir.clone())?;
    for (rel, _) in workspace.file_status() {
        if created.contains(&rel) {
            println!("Wrote {}", workspace.dir.join(rel).display());
        } else {
            println!("Kept  {} (already exists)", workspace.dir.join(rel).display());
        }
    }

    println!("\nNext steps:");
    println!("  1. Edit resources.cv_path to point at your CV (markdown/plain text).");
    println!("  2. Adjust criteria to your preferences.");
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

    let cv_path = expand_tilde(&config.resources.cv_path);
    match std::fs::read_to_string(&cv_path) {
        Ok(text) => {
            let chars = text.chars().count();
            let mode = if chars <= config.assessment.cv_verbatim_max_chars {
                "embedded verbatim (lossless)"
            } else {
                "will be distilled once (see assessment.cv_digest_max_words)"
            };
            println!("cv:       {} ({chars} chars - {mode})", cv_path.display());
        }
        Err(e) => println!("cv:       {} - NOT READABLE: {e}", cv_path.display()),
    }
    // The supplements travel with every assessment, so a typo'd path is worth
    // catching here rather than at the next `assess`.
    for (label, configured) in [
        ("background:", config.resources.background_path.as_deref()),
        ("wants:", config.resources.preferences_path.as_deref()),
    ] {
        let Some(path) = configured else { continue };
        let expanded = expand_tilde(path);
        match std::fs::read_to_string(&expanded) {
            Ok(text) => {
                println!("  + {label:<11} {} ({} chars)", expanded.display(), text.chars().count())
            }
            Err(e) => println!("  + {label:<11} {} - NOT READABLE: {e}", expanded.display()),
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
        "criteria: {} hard filter(s), {} soft criteria - hash {}",
        config.criteria.hard_filters.len(),
        config.criteria.soft.len(),
        config.criteria_hash()
    );
    println!("\nConfig is valid.");
    Ok(())
}
