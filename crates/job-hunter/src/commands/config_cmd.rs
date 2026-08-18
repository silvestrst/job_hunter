use anyhow::{bail, Context, Result};
use job_hunter_core::config::{Config, DEFAULT_CONFIG_YAML};
use job_hunter_core::paths::{expand_tilde, Paths};
use job_hunter_core::token;

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
    println!("\nNext steps:");
    println!("  1. Edit resources.cv_path to point at your CV (markdown/plain text).");
    println!("  2. Adjust criteria to your preferences.");
    println!("  3. `job-hunter config check` to validate.");
    println!("  4. `job-hunter serve` and load the browser extension.");
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
