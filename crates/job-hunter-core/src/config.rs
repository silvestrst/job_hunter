use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    pub resources: ResourcesConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub assessment: AssessmentConfig,
    #[serde(default)]
    pub criteria: CriteriaConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourcesConfig {
    /// Everything the agent should know about the candidate - experience,
    /// skills, project scope and context - as one markdown/plain text file
    /// (`~/` is expanded). Deliberately not "the CV": nothing here is
    /// summarised or truncated before it reaches a prompt, so write the full
    /// detail rather than a document formatted to fit a page.
    pub experience_path: String,
    /// Optional: what the candidate wants from a role, in their own words.
    /// Deliberately advisory - it earns its own holistic grade
    /// (`fit.preference_match`) and shapes the agent's prose, but never the
    /// extracted facts, the fit measures, or the score, which answers how well
    /// the candidate fits the role rather than how much they want it.
    /// Dealbreakers belong in `criteria.hard_filters`, which needs no LLM.
    #[serde(default)]
    pub preferences_path: Option<String>,
    /// The one size limit, and a safety guard rather than a token lever: an
    /// input file bigger than this is refused unread, so a mistyped path
    /// pointing at something huge can never reach a prompt or a bill. Checked
    /// against the file's size on disk before it is opened - for markdown
    /// that tracks its character count to within a fraction of a percent.
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    /// Argv template; run directly, never through a shell. If any element
    /// contains `{prompt}` it is substituted, otherwise the prompt is piped
    /// to the child's stdin. If any element contains `{model}` it is
    /// substituted with `model` below.
    #[serde(default = "default_agent_command")]
    pub command: Vec<String>,
    /// Explicit model id substituted into `{model}` in `command`. Required
    /// iff `command` references `{model}` - see `Config::validate`. Without
    /// this, the agent CLI falls back to whatever model it's configured to
    /// use by default, which may not be the one you want to pay for.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub output: AgentOutput,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_retries")]
    pub retries: u32,
    /// Directory the agent runs in (its cwd), holding the instruction files -
    /// AGENTS.md, CLAUDE.md, and per-task skills (`~/` is expanded). Defaults
    /// to `<config dir>/agent`; missing files are created from the built-in
    /// defaults and never overwritten once present.
    #[serde(default)]
    pub workspace_dir: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutput {
    /// stdout is a Claude Code `--output-format json` envelope; unwrap `.result`.
    #[default]
    ClaudeJson,
    /// stdout is the answer itself.
    Text,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssessmentConfig {
    #[serde(default = "default_prompt_version")]
    pub prompt_version: i64,
    /// Job postings arrive from the browser extension, not from a file the
    /// user controls, so this stays a truncation budget rather than a refusal.
    #[serde(default = "default_max_job_chars")]
    pub max_job_chars: usize,
    #[serde(default = "default_feedback_recent_n")]
    pub feedback_recent_n: usize,
}

/// Layer-2 criteria: filtering, and nothing else. Deliberately serializable:
/// the criteria hash is the sha256 of this struct's canonical JSON, so cached
/// filter verdicts key off meaning, not YAML formatting. BTreeMap keeps map
/// ordering deterministic.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CriteriaConfig {
    #[serde(default)]
    pub currency_rates_to_gbp: BTreeMap<String, f64>,
    #[serde(default)]
    pub hard_filters: Vec<HardFilter>,
    /// Accepted only so a config written before scoring moved to the two fit
    /// measures gets an explanation instead of "unknown field `soft`". Never
    /// serialized, so its presence cannot change `criteria_hash`; `validate`
    /// refuses it outright rather than ignoring weights the user still
    /// believes in.
    #[serde(default, skip_serializing)]
    pub soft: Option<serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HardFilter {
    pub fact: String,
    pub op: FilterOp,
    pub value: serde_json::Value,
    #[serde(default)]
    pub if_missing: MissingPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    Gte,
    Lte,
    Eq,
    In,
    NotIn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingPolicy {
    #[default]
    Pass,
    Fail,
}

fn default_port() -> u16 {
    7877
}
fn default_agent_command() -> Vec<String> {
    vec![
        "claude".into(),
        "-p".into(),
        "--output-format".into(),
        "json".into(),
        "--model".into(),
        "{model}".into(),
    ]
}
fn default_agent_model() -> &'static str {
    "claude-sonnet-5"
}
fn default_timeout_secs() -> u64 {
    180
}
fn default_retries() -> u32 {
    2
}
fn default_prompt_version() -> i64 {
    1
}
fn default_max_job_chars() -> usize {
    12_000
}
/// ~8000 characters of markdown. Comfortably above a thorough experience
/// document, far below anything you would reference by accident.
fn default_max_file_bytes() -> u64 {
    8_192
}
fn default_feedback_recent_n() -> usize {
    10
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { port: default_port() }
    }
}
impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            command: default_agent_command(),
            model: Some(default_agent_model().into()),
            output: AgentOutput::default(),
            timeout_secs: default_timeout_secs(),
            retries: default_retries(),
            workspace_dir: None,
        }
    }
}
impl Default for AssessmentConfig {
    fn default() -> Self {
        Self {
            prompt_version: default_prompt_version(),
            max_job_chars: default_max_job_chars(),
            feedback_recent_n: default_feedback_recent_n(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read config file {}", path.display()))?;
        let config: Config = serde_yaml::from_str(&text)
            .with_context(|| format!("invalid config file {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    /// Structural validation beyond serde: agent command non-empty and every
    /// criteria fact name resolvable. Fails fast at load, not at scoring time.
    pub fn validate(&self) -> Result<()> {
        if self.agent.command.is_empty() {
            bail!("agent.command must not be empty");
        }
        let references_model = self.agent.command.iter().any(|a| a.contains("{model}"));
        match (references_model, &self.agent.model) {
            (true, None) => {
                bail!("agent.command references {{model}} but agent.model is not set")
            }
            (false, Some(_)) => bail!(
                "agent.model is set but agent.command does not reference {{model}} - \
                 add {{model}} to agent.command or remove agent.model"
            ),
            _ => {}
        }
        if self.criteria.soft.is_some() {
            bail!(
                "criteria.soft is no longer supported - the score is now the mean of the \
                 agent's overall_cv_match and domain_familiarity, and criteria only filter. \
                 Delete the `soft:` section; your hard_filters keep working unchanged"
            );
        }
        for f in &self.criteria.hard_filters {
            crate::scoring::FactRef::parse(&f.fact)
                .with_context(|| format!("in criteria.hard_filters, fact '{}'", f.fact))?;
        }
        Ok(())
    }

    /// 16-hex-char digest of the parsed criteria: whitespace/comment edits in
    /// the YAML do not change it, semantic edits do.
    pub fn criteria_hash(&self) -> String {
        let canonical =
            serde_json::to_string(&self.criteria).expect("criteria serialization cannot fail");
        crate::sha256_hex(canonical.as_bytes())[..16].to_string()
    }
}

/// Commented default config written by `config init`.
pub const DEFAULT_CONFIG_YAML: &str = r#"# job-hunter configuration
# Location: ~/.config/job-hunter/config.yaml

server:
  port: 7877                       # ingest server; binds 127.0.0.1 only

resources:
  # Everything about you, as one markdown or plain text file: roles, projects,
  # scope, skills, context. Not a formatted CV - nothing here is summarised or
  # cut before it reaches the agent, so write the full detail.
  experience_path: ~/Documents/experience.md
  # Optional, also markdown/plain text. What you want from your next role, in
  # your own words. Advisory: the agent grades the whole posting against it
  # (poor..exceptional, shown in `jobs` and `show`) and it shapes the flags,
  # notes and suggestions - but it never touches the extracted facts, the fit
  # measures, or the score. Dealbreakers belong in criteria.hard_filters below:
  # preferences_path: ~/Documents/preferences.md
  # Safety guard on the files above: anything larger is refused unread, so a
  # mistyped path can't put something huge in a prompt. ~8000 characters of
  # markdown; raise it if your experience file legitimately outgrows it.
  max_file_bytes: 8192

agent:
  # LLM agent command as an argv array (no shell). If an element contains
  # "{prompt}" it is substituted; otherwise the prompt is piped to stdin.
  # If an element contains "{model}" it is substituted with `model` below.
  # Swap in any other agent CLI, e.g.:
  #   command: ["codex", "exec", "-m", "{model}", "--quiet", "{prompt}"]
  #   output: text
  command: ["claude", "-p", "--output-format", "json", "--model", "{model}"]
  # Pin the model explicitly. Without this, the CLI falls back to whatever
  # model it's configured to use by default - which may not be the one you
  # want to pay for. Required iff command references "{model}" above.
  model: claude-sonnet-5
  output: claude_json              # claude_json = unwrap Claude's JSON envelope; text = stdout as-is
  timeout_secs: 180
  retries: 2
  # The agent runs inside the agent workspace (default ~/.config/job-hunter/agent),
  # which holds its instructions: AGENTS.md, CLAUDE.md and one skill per task.
  # Edit those files to tune behaviour - edits re-run affected assessments on
  # the next `assess` (spends tokens). Uncomment to relocate the workspace:
  # workspace_dir: ~/.config/job-hunter/agent

assessment:
  prompt_version: 1                # bump to force re-assessment of every job
  max_job_chars: 12000             # job text is truncated beyond this
  feedback_recent_n: 10            # recent feedback items included in prompts

criteria:
  # Layer 2: filtering, and only filtering. Editing this section never re-runs
  # the LLM - filter verdicts recompute instantly.
  #
  # There are no weights here. The score is the mean of the agent's two fit
  # measures, overall_cv_match and domain_familiarity, and nothing else moves
  # it. Every other fact - salary, seniority, work mode - is informational:
  # shown in `jobs` and `show`, and available to filter on below.
  #
  # A filter is pass/fail. Failures are hidden from the default `jobs` view
  # (counted in its summary line, shown with --include-filtered), never scored
  # down.
  #
  # Valid fact names: salary_max_gbp, salary_min_gbp, work_mode,
  # employment_type, seniority_level, seniority_fit, overall_cv_match,
  # domain_familiarity, preference_match, visa_sponsorship, country,
  # skill:<name>
  currency_rates_to_gbp:
    USD: 0.78
    EUR: 0.86
  hard_filters:
    - {fact: salary_max_gbp, op: gte, value: 85000, if_missing: pass}
    - {fact: work_mode, op: in, value: [remote, hybrid], if_missing: fail}
    # ops: gte, lte, eq, in, not_in - if_missing: pass|fail
    # - {fact: overall_cv_match, op: gte, value: 0.5}     # fit measures are 0..1
    # - {fact: preference_match, op: not_in, value: [poor], if_missing: pass}
    # - {fact: "skill:rust", op: in, value: [strong, moderate], if_missing: fail}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses_and_validates() {
        let config: Config = serde_yaml::from_str(DEFAULT_CONFIG_YAML).unwrap();
        config.validate().unwrap();
        assert_eq!(config.server.port, 7877);
        assert_eq!(config.agent.output, AgentOutput::ClaudeJson);
        assert_eq!(config.agent.model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(config.criteria.hard_filters.len(), 2);
    }

    #[test]
    fn omitted_agent_section_defaults_to_pinned_model() {
        let config: Config = serde_yaml::from_str(
            "resources:\n  experience_path: ~/experience.md\n",
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(config.agent.model.as_deref(), Some("claude-sonnet-5"));
        assert!(config.agent.command.iter().any(|a| a.contains("{model}")));
    }

    #[test]
    fn preferences_path_is_optional() {
        let config: Config = serde_yaml::from_str(DEFAULT_CONFIG_YAML).unwrap();
        assert!(config.resources.preferences_path.is_none());

        let yaml = DEFAULT_CONFIG_YAML
            .replace("  # preferences_path: ~/Documents/preferences.md", "  preferences_path: ~/want.md");
        let config: Config = serde_yaml::from_str(&yaml).unwrap();
        config.validate().unwrap();
        assert_eq!(config.resources.preferences_path.as_deref(), Some("~/want.md"));
    }

    #[test]
    fn max_file_bytes_defaults_when_omitted() {
        // The guard must exist even in a config that predates it, otherwise
        // upgrading would silently leave input files unchecked.
        let config: Config = serde_yaml::from_str(
            "resources:\n  experience_path: ~/experience.md\n",
        )
        .unwrap();
        assert_eq!(config.resources.max_file_bytes, 8_192);
        let config: Config = serde_yaml::from_str(DEFAULT_CONFIG_YAML).unwrap();
        assert_eq!(config.resources.max_file_bytes, 8_192);
    }

    #[test]
    fn workspace_dir_override_parses() {
        assert!(
            serde_yaml::from_str::<Config>(DEFAULT_CONFIG_YAML)
                .unwrap()
                .agent
                .workspace_dir
                .is_none()
        );
        let yaml = DEFAULT_CONFIG_YAML.replace(
            "  # workspace_dir: ~/.config/job-hunter/agent",
            "  workspace_dir: ~/custom-agent",
        );
        let config: Config = serde_yaml::from_str(&yaml).unwrap();
        config.validate().unwrap();
        assert_eq!(config.agent.workspace_dir.as_deref(), Some("~/custom-agent"));
    }

    #[test]
    fn rejects_model_placeholder_without_model() {
        let bad = DEFAULT_CONFIG_YAML.replace("  model: claude-sonnet-5\n", "");
        let config: Config = serde_yaml::from_str(&bad).unwrap();
        let err = config.validate().unwrap_err();
        assert!(format!("{err:#}").contains("agent.model is not set"));
    }

    #[test]
    fn rejects_model_without_placeholder() {
        let bad = DEFAULT_CONFIG_YAML.replace(
            r#"command: ["claude", "-p", "--output-format", "json", "--model", "{model}"]"#,
            r#"command: ["claude", "-p", "--output-format", "json"]"#,
        );
        let config: Config = serde_yaml::from_str(&bad).unwrap();
        let err = config.validate().unwrap_err();
        assert!(format!("{err:#}").contains("does not reference {model}"));
    }

    #[test]
    fn criteria_hash_ignores_formatting_but_not_meaning() {
        let a: Config = serde_yaml::from_str(DEFAULT_CONFIG_YAML).unwrap();
        // Same criteria, different YAML formatting.
        let reformatted = DEFAULT_CONFIG_YAML.replace(
            "- {fact: salary_max_gbp, op: gte, value: 85000, if_missing: pass}",
            "- fact: salary_max_gbp\n      op: gte\n      value: 85000\n      if_missing: pass",
        );
        let b: Config = serde_yaml::from_str(&reformatted).unwrap();
        assert_eq!(a.criteria_hash(), b.criteria_hash());

        let changed = DEFAULT_CONFIG_YAML.replace("value: 85000", "value: 95000");
        let c: Config = serde_yaml::from_str(&changed).unwrap();
        assert_ne!(a.criteria_hash(), c.criteria_hash());
    }

    #[test]
    fn unknown_fact_rejected() {
        let bad = DEFAULT_CONFIG_YAML.replace("fact: salary_max_gbp", "fact: nonsense_fact");
        let config: Config = serde_yaml::from_str(&bad).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn a_leftover_soft_section_is_explained_not_ignored() {
        // Silently dropping weights the user still believes in would be the
        // one failure mode worse than refusing to start.
        let yaml = format!("{DEFAULT_CONFIG_YAML}  soft:\n    - {{fact: overall_cv_match, weight: 5}}\n");
        let config: Config = serde_yaml::from_str(&yaml).unwrap();
        let err = format!("{:#}", config.validate().unwrap_err());
        assert!(err.contains("no longer supported"), "{err}");
        assert!(err.contains("mean of"), "{err}");
        assert!(err.contains("hard_filters keep working"), "{err}");

        // And it cannot quietly change what scores are cached against.
        let clean: Config = serde_yaml::from_str(DEFAULT_CONFIG_YAML).unwrap();
        assert_eq!(config.criteria_hash(), clean.criteria_hash());
    }
}
