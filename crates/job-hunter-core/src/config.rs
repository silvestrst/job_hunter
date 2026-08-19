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
    /// Path to the CV as markdown/plain text (`~/` is expanded).
    pub cv_path: String,
    /// Optional: detail on experience and skills the CV has no room for -
    /// project scope, depth, context. Read as further evidence about the
    /// candidate, exactly like the CV itself.
    #[serde(default)]
    pub background_path: Option<String>,
    /// Optional: what the candidate wants from a role, in their own words.
    /// Deliberately advisory - it shapes the agent's prose (flags, notes,
    /// suggestions) but never the extracted facts or the fit numbers, because
    /// hard preferences belong in `criteria`, which is scored without an LLM.
    #[serde(default)]
    pub preferences_path: Option<String>,
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
    #[serde(default = "default_max_job_chars")]
    pub max_job_chars: usize,
    /// A CV at or under this many characters is embedded verbatim (lossless).
    #[serde(default = "default_cv_verbatim_max_chars")]
    pub cv_verbatim_max_chars: usize,
    /// Word budget for the one-off distillation of a CV over the verbatim limit.
    #[serde(default = "default_cv_digest_max_words")]
    pub cv_digest_max_words: usize,
    /// Budget for each supplementary profile document in a prompt.
    #[serde(default = "default_max_profile_chars")]
    pub max_profile_chars: usize,
    #[serde(default = "default_feedback_recent_n")]
    pub feedback_recent_n: usize,
}

/// Layer-2 criteria. Deliberately serializable: the criteria hash is the
/// sha256 of this struct's canonical JSON, so score caching keys off meaning,
/// not YAML formatting. BTreeMap keeps map ordering deterministic.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CriteriaConfig {
    #[serde(default)]
    pub currency_rates_to_gbp: BTreeMap<String, f64>,
    #[serde(default)]
    pub hard_filters: Vec<HardFilter>,
    #[serde(default)]
    pub soft: Vec<SoftCriterion>,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SoftCriterion {
    pub fact: String,
    pub weight: f64,
    /// For numeric facts: sub-score = clamp(value / target, 0, 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<f64>,
    /// Optional explicit mapping for enum facts, e.g. {remote: 1.0, hybrid: 0.6}.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<BTreeMap<String, f64>>,
    /// Sub-score used when the fact is missing/unspecified (default 0.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_score: Option<f64>,
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
fn default_cv_verbatim_max_chars() -> usize {
    6_000
}
fn default_cv_digest_max_words() -> usize {
    800
}
fn default_max_profile_chars() -> usize {
    6_000
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
            cv_verbatim_max_chars: default_cv_verbatim_max_chars(),
            cv_digest_max_words: default_cv_digest_max_words(),
            max_profile_chars: default_max_profile_chars(),
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
        for f in &self.criteria.hard_filters {
            crate::scoring::FactRef::parse(&f.fact)
                .with_context(|| format!("in criteria.hard_filters, fact '{}'", f.fact))?;
        }
        for s in &self.criteria.soft {
            let fact = crate::scoring::FactRef::parse(&s.fact)
                .with_context(|| format!("in criteria.soft, fact '{}'", s.fact))?;
            fact.check_soft(s)?;
            if s.weight <= 0.0 {
                bail!("criteria.soft fact '{}' must have a positive weight", s.fact);
            }
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
  # Your CV as markdown or plain text (export it once from your source doc).
  cv_path: ~/Documents/cv.md
  # Optional extras, also markdown/plain text. Both travel with every
  # assessment and are the raw material for `job-hunter cv-advice`.
  # Depth the CV has no room for - project scope, context, skills detail.
  # Read as further evidence about you, exactly like the CV itself:
  # background_path: ~/Documents/background.md
  # What you want from your next role, in your own words. Advisory only: it
  # shapes the agent's flags, notes and suggestions, never the extracted
  # facts or the fit numbers. Hard preferences belong in `criteria` below,
  # which is scored without an LLM:
  # preferences_path: ~/Documents/preferences.md

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
  cv_verbatim_max_chars: 6000      # CV at or under this is embedded verbatim (lossless)
  cv_digest_max_words: 800         # word budget when a longer CV is distilled once
  max_profile_chars: 6000          # background/preferences docs truncated beyond this
  feedback_recent_n: 10            # recent feedback items included in prompts

criteria:
  # Layer-2 scoring: pure logic over stored assessment facts. Editing this
  # section never re-runs the LLM - scores recompute instantly.
  #
  # Valid fact names: salary_max_gbp, salary_min_gbp, work_mode,
  # employment_type, seniority_level, seniority_fit, overall_cv_match,
  # domain_familiarity, visa_sponsorship, country, skill:<name>
  currency_rates_to_gbp:
    USD: 0.78
    EUR: 0.86
  hard_filters:
    - {fact: salary_max_gbp, op: gte, value: 85000, if_missing: pass}
    - {fact: work_mode, op: in, value: [remote, hybrid], if_missing: fail}
  soft:
    - {fact: overall_cv_match, weight: 5}
    - {fact: seniority_fit, weight: 3}
    - {fact: salary_max_gbp, weight: 3, target: 120000}
    - {fact: domain_familiarity, weight: 1}
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
            "resources:\n  cv_path: ~/cv.md\n",
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(config.agent.model.as_deref(), Some("claude-sonnet-5"));
        assert!(config.agent.command.iter().any(|a| a.contains("{model}")));
    }

    #[test]
    fn supplementary_profile_paths_are_optional() {
        let config: Config = serde_yaml::from_str(DEFAULT_CONFIG_YAML).unwrap();
        assert!(config.resources.background_path.is_none());
        assert!(config.resources.preferences_path.is_none());

        let yaml = DEFAULT_CONFIG_YAML
            .replace("  # background_path: ~/Documents/background.md", "  background_path: ~/bg.md")
            .replace("  # preferences_path: ~/Documents/preferences.md", "  preferences_path: ~/want.md");
        let config: Config = serde_yaml::from_str(&yaml).unwrap();
        config.validate().unwrap();
        assert_eq!(config.resources.background_path.as_deref(), Some("~/bg.md"));
        assert_eq!(config.resources.preferences_path.as_deref(), Some("~/want.md"));
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
            "- {fact: overall_cv_match, weight: 5}",
            "- fact: overall_cv_match\n      weight: 5",
        );
        let b: Config = serde_yaml::from_str(&reformatted).unwrap();
        assert_eq!(a.criteria_hash(), b.criteria_hash());

        let changed = DEFAULT_CONFIG_YAML.replace("weight: 5", "weight: 4");
        let c: Config = serde_yaml::from_str(&changed).unwrap();
        assert_ne!(a.criteria_hash(), c.criteria_hash());
    }

    #[test]
    fn unknown_fact_rejected() {
        let bad = DEFAULT_CONFIG_YAML.replace("fact: overall_cv_match", "fact: nonsense_fact");
        let config: Config = serde_yaml::from_str(&bad).unwrap();
        assert!(config.validate().is_err());
    }
}
