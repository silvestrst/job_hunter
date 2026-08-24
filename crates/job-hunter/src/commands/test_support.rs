//! Shared scaffolding for the command tests: a `Ctx` rooted in a temp dir and
//! pointed at whichever mock agent the test needs. The whole CLI is exercised
//! offline this way - `fixtures/mock-agent.sh` and friends stand in for the
//! LLM, so no test ever spends a token.

use super::Ctx;
use job_hunter_core::config::Config;
use job_hunter_core::db::Db;
use job_hunter_core::ingest::ScrapedPayload;
use job_hunter_core::paths::Paths;
use std::path::{Path, PathBuf};

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").canonicalize().unwrap()
}

/// The named mock agent as an argv, with `MOCK_CAPTURE_FILE` set when the test
/// wants to assert on the prompts it received.
pub fn mock_agent(name: &str, capture: Option<&Path>) -> Vec<String> {
    let script = fixtures_dir().join(name).to_string_lossy().into_owned();
    match capture {
        Some(path) => {
            vec!["env".into(), format!("MOCK_CAPTURE_FILE={}", path.display()), script]
        }
        None => vec![script],
    }
}

/// Ctx over a temp dir, configured with the given agent argv.
pub fn test_ctx(dir: &Path, agent_command: &[String]) -> Ctx {
    test_ctx_with_resources(dir, agent_command, "")
}

/// `extra_resources` is appended verbatim under `resources:`.
pub fn test_ctx_with_resources(dir: &Path, agent_command: &[String], extra_resources: &str) -> Ctx {
    let fixtures = fixtures_dir();
    let command_yaml: Vec<String> = agent_command.iter().map(|c| format!("{c:?}")).collect();
    let config_yaml = format!(
        r#"
resources:
  experience_path: {experience}
{extra_resources}
agent:
  command: [{command}]
  output: claude_json
  timeout_secs: 30
  retries: 2
criteria:
  currency_rates_to_gbp: {{USD: 0.78, EUR: 0.86}}
  hard_filters:
    - {{fact: salary_max_gbp, op: gte, value: 85000, if_missing: pass}}
    - {{fact: work_mode, op: in, value: [remote, hybrid], if_missing: fail}}
  soft:
    - {{fact: overall_cv_match, weight: 5}}
    - {{fact: seniority_fit, weight: 3}}
    - {{fact: salary_max_gbp, weight: 3, target: 120000}}
    - {{fact: domain_familiarity, weight: 1}}
"#,
        experience = fixtures.join("sample-experience.md").display(),
        command = command_yaml.join(", "),
    );
    let config_file = dir.join("config.yaml");
    std::fs::write(&config_file, config_yaml).unwrap();
    let config = Config::load(&config_file).unwrap();
    let db = Db::open(&dir.join("job_hunter.db")).unwrap();
    Ctx {
        paths: Paths {
            config_file,
            data_dir: dir.to_path_buf(),
            db_file: dir.join("job_hunter.db"),
            api_token_file: dir.join("api_token"),
            agent_workspace_dir: dir.join("agent"),
            systemd_unit_file: dir.join("job-hunter.service"),
        },
        config,
        db,
    }
}

pub fn load_payload(name: &str) -> ScrapedPayload {
    let text = std::fs::read_to_string(fixtures_dir().join("payloads").join(name)).unwrap();
    serde_json::from_str(&text).unwrap()
}
