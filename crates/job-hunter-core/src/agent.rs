//! LLM-agnostic agent invocation: run the configured argv (no shell), feed the
//! prompt via `{prompt}` substitution or stdin, unwrap the configured output
//! shape, then defensively extract/validate the facts JSON with retries.

use crate::assessment::schema::AssessmentFacts;
use crate::config::{AgentConfig, AgentOutput};
use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

pub struct AgentRunner<'a> {
    config: &'a AgentConfig,
}

impl<'a> AgentRunner<'a> {
    pub fn new(config: &'a AgentConfig) -> Self {
        Self { config }
    }

    /// Run the agent once and return its answer as plain text (envelope
    /// already unwrapped). Used for CV digestion and feedback compaction.
    pub fn run_text(&self, prompt: &str) -> Result<String> {
        let stdout = self.exec(prompt)?;
        Ok(self.unwrap_output(&stdout)?.trim().to_string())
    }

    /// Run the agent expecting facts JSON, retrying with a corrective note on
    /// parse/validation failure. Returns the typed facts plus the exact JSON
    /// stored in the database.
    pub fn run_facts(&self, base_prompt: &str) -> Result<(AssessmentFacts, String)> {
        let attempts = self.config.retries + 1;
        let mut last_err: Option<anyhow::Error> = None;
        for _ in 0..attempts {
            let prompt = match &last_err {
                None => base_prompt.to_string(),
                Some(e) => format!(
                    "{base_prompt}\n\nYour previous output failed validation: {e:#}.\n\
                     Return only the corrected JSON object."
                ),
            };
            match self.try_facts(&prompt) {
                Ok(ok) => return Ok(ok),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.expect("at least one attempt ran"))
            .with_context(|| format!("agent failed after {attempts} attempt(s)"))
    }

    fn try_facts(&self, prompt: &str) -> Result<(AssessmentFacts, String)> {
        let answer = self.run_text(prompt)?;
        let json = extract_json_block(&answer)?;
        let facts: AssessmentFacts =
            serde_json::from_str(json).context("agent JSON does not match the facts schema")?;
        facts.validate()?;
        Ok((facts, json.to_string()))
    }

    fn exec(&self, prompt: &str) -> Result<String> {
        let mut argv = self.config.command.clone();
        let mut used_placeholder = false;
        for arg in &mut argv {
            if arg.contains("{prompt}") {
                *arg = arg.replace("{prompt}", prompt);
                used_placeholder = true;
            }
        }
        if let Some(model) = &self.config.model {
            for arg in &mut argv {
                if arg.contains("{model}") {
                    *arg = arg.replace("{model}", model);
                }
            }
        }
        let program = argv.first().context("agent.command is empty")?.clone();

        let mut cmd = Command::new(&program);
        cmd.args(&argv[1..])
            .stdin(if used_placeholder { Stdio::null() } else { Stdio::piped() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .with_context(|| format!("cannot run agent command '{program}' - is it installed?"))?;

        // Writer/reader threads prevent pipe-buffer deadlock on large prompts/outputs.
        let stdin_thread = if !used_placeholder {
            let mut stdin = child.stdin.take().expect("stdin was piped");
            let text = prompt.to_string();
            Some(std::thread::spawn(move || {
                let _ = stdin.write_all(text.as_bytes());
            }))
        } else {
            None
        };
        let mut stdout_pipe = child.stdout.take().expect("stdout was piped");
        let stdout_thread = std::thread::spawn(move || {
            let mut buf = String::new();
            let _ = stdout_pipe.read_to_string(&mut buf);
            buf
        });
        let mut stderr_pipe = child.stderr.take().expect("stderr was piped");
        let stderr_thread = std::thread::spawn(move || {
            let mut buf = String::new();
            let _ = stderr_pipe.read_to_string(&mut buf);
            buf
        });

        let status = match child
            .wait_timeout(Duration::from_secs(self.config.timeout_secs))
            .context("waiting for agent process")?
        {
            Some(status) => status,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("agent timed out after {}s", self.config.timeout_secs);
            }
        };
        if let Some(t) = stdin_thread {
            let _ = t.join();
        }
        let stdout = stdout_thread.join().expect("stdout reader panicked");
        let stderr = stderr_thread.join().expect("stderr reader panicked");

        if !status.success() {
            bail!("agent exited with {status}: {}", stderr.trim());
        }
        Ok(stdout)
    }

    fn unwrap_output(&self, stdout: &str) -> Result<String> {
        match self.config.output {
            AgentOutput::Text => Ok(stdout.to_string()),
            AgentOutput::ClaudeJson => {
                let envelope = extract_json_block(stdout)
                    .ok()
                    .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok());
                match envelope {
                    Some(v) => {
                        if v.get("is_error").and_then(|e| e.as_bool()) == Some(true) {
                            bail!(
                                "agent reported an error: {}",
                                v.get("result").and_then(|r| r.as_str()).unwrap_or("unknown")
                            );
                        }
                        match v.get("result").and_then(|r| r.as_str()) {
                            Some(result) => Ok(result.to_string()),
                            // Envelope shape drifted - fall back to raw stdout.
                            None => Ok(stdout.to_string()),
                        }
                    }
                    None => Ok(stdout.to_string()),
                }
            }
        }
    }
}

/// First balanced `{...}` block in the text; tolerates markdown fences and
/// prose around the JSON.
pub fn extract_json_block(text: &str) -> Result<&str> {
    let start = text.find('{').context("no JSON object in agent output")?;
    let bytes = text.as_bytes();
    let (mut depth, mut in_str, mut escape) = (0usize, false, false);
    for (offset, &b) in bytes[start..].iter().enumerate() {
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
        } else {
            match b {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(&text[start..=start + offset]);
                    }
                }
                _ => {}
            }
        }
    }
    bail!("unbalanced JSON object in agent output")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentConfig;

    #[test]
    fn extracts_json_from_fenced_output() {
        let text = "Sure! Here it is:\n```json\n{\"a\": {\"b\": \"}\"}, \"c\": 1}\n```\ndone";
        assert_eq!(extract_json_block(text).unwrap(), "{\"a\": {\"b\": \"}\"}, \"c\": 1}");
    }

    #[test]
    fn rejects_unbalanced() {
        assert!(extract_json_block("{\"a\": 1").is_err());
        assert!(extract_json_block("no json here").is_err());
    }

    fn shell_agent(script: &str, output: AgentOutput) -> AgentConfig {
        AgentConfig {
            command: vec!["sh".into(), "-c".into(), script.into()],
            model: None,
            output,
            timeout_secs: 10,
            retries: 1,
        }
    }

    #[test]
    fn runs_text_agent_via_stdin() {
        // `cat` echoes the prompt back: proves stdin piping works.
        let config = shell_agent("cat", AgentOutput::Text);
        let out = AgentRunner::new(&config).run_text("hello prompt").unwrap();
        assert_eq!(out, "hello prompt");
    }

    #[test]
    fn substitutes_prompt_placeholder() {
        let config = AgentConfig {
            command: vec!["printf".into(), "%s".into(), "got:{prompt}".into()],
            model: None,
            output: AgentOutput::Text,
            timeout_secs: 10,
            retries: 0,
        };
        let out = AgentRunner::new(&config).run_text("X").unwrap();
        assert_eq!(out, "got:X");
    }

    #[test]
    fn substitutes_model_placeholder() {
        let config = AgentConfig {
            command: vec!["printf".into(), "%s".into(), "model={model} prompt={prompt}".into()],
            model: Some("claude-sonnet-5".into()),
            output: AgentOutput::Text,
            timeout_secs: 10,
            retries: 0,
        };
        let out = AgentRunner::new(&config).run_text("X").unwrap();
        assert_eq!(out, "model=claude-sonnet-5 prompt=X");
    }

    #[test]
    fn unwraps_claude_envelope() {
        let config = shell_agent(
            r#"printf '%s' '{"type":"result","is_error":false,"result":"{\"schema_version\":1}"}'"#,
            AgentOutput::ClaudeJson,
        );
        let (facts, json) = AgentRunner::new(&config).run_facts("p").unwrap();
        assert_eq!(facts.schema_version, 1);
        assert_eq!(json, "{\"schema_version\":1}");
    }

    #[test]
    fn nonzero_exit_is_error() {
        let config = shell_agent("echo boom >&2; exit 3", AgentOutput::Text);
        let err = AgentRunner::new(&config).run_text("p").unwrap_err();
        assert!(format!("{err:#}").contains("boom"));
    }

    #[test]
    fn timeout_kills_agent() {
        let mut config = shell_agent("sleep 30", AgentOutput::Text);
        config.timeout_secs = 1;
        let err = AgentRunner::new(&config).run_text("p").unwrap_err();
        assert!(format!("{err:#}").contains("timed out"));
    }
}
