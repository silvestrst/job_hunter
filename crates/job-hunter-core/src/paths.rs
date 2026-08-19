use anyhow::{Context, Result};
use std::path::PathBuf;

/// Resolved filesystem locations following the XDG conventions.
pub struct Paths {
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
    pub db_file: PathBuf,
    pub api_token_file: PathBuf,
    pub cv_digest_dir: PathBuf,
    /// Default agent workspace (instruction files); user-editable, so it
    /// lives with the config rather than the data.
    pub agent_workspace_dir: PathBuf,
    pub systemd_unit_file: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Self> {
        let proj = directories::ProjectDirs::from("", "", "job-hunter")
            .context("cannot determine home directory")?;
        let config_dir = proj.config_dir().to_path_buf(); // ~/.config/job-hunter
        let data_dir = proj.data_dir().to_path_buf(); // ~/.local/share/job-hunter
        let base = directories::BaseDirs::new().context("cannot determine home directory")?;
        Ok(Self {
            config_file: config_dir.join("config.yaml"),
            db_file: data_dir.join("job_hunter.db"),
            api_token_file: data_dir.join("api_token"),
            cv_digest_dir: data_dir.join("cv_digest"),
            agent_workspace_dir: config_dir.join("agent"),
            systemd_unit_file: base
                .home_dir()
                .join(".config/systemd/user/job-hunter.service"),
            data_dir,
        })
    }
}

/// Expand a leading `~/` to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(base) = directories::BaseDirs::new() {
            return base.home_dir().join(rest);
        }
    }
    PathBuf::from(path)
}
