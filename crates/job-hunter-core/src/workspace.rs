//! The agent workspace: the directory the agent CLI runs in, holding its
//! instructions as standard conventions - AGENTS.md (source of truth),
//! CLAUDE.md (an `@AGENTS.md` pointer for Claude Code), and one Agent Skill
//! per task. Defaults ship embedded in the binary and are written out
//! create-if-missing, so user edits survive; the instructions in effect are
//! hashed into each assessment so edits invalidate exactly like a
//! `prompt_version` bump.

use crate::config::AgentConfig;
use crate::paths::{expand_tilde, Paths};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// One LLM task; its instructions live in the matching SKILL.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTask {
    Assess,
    FeedbackCompact,
}

impl AgentTask {
    pub fn skill_name(&self) -> &'static str {
        match self {
            AgentTask::Assess => "assess",
            AgentTask::FeedbackCompact => "feedback-compact",
        }
    }

    /// Path of the task's SKILL.md relative to the workspace root.
    pub fn skill_path(&self) -> String {
        format!(".claude/skills/{}/SKILL.md", self.skill_name())
    }

    /// The prompt line pointing an agent at this task's instructions. Names
    /// the skill and gives its path, because agents resolve one or the other:
    /// a skill-aware CLI (Claude Code) matches the name against the skills it
    /// discovered in its cwd, while a plain argv agent can only read a file.
    pub fn instructions_pointer(&self) -> String {
        format!(
            "Use the `{}` skill ({} in your working directory); \
             AGENTS.md has the conventions.",
            self.skill_name(),
            self.skill_path()
        )
    }
}

/// (relative path, embedded default) for every workspace file. The assets
/// directory mirrors the workspace layout exactly; a test asserts the two
/// stay in sync so a new asset can't silently fail to ship.
const DEFAULT_FILES: &[(&str, &str)] = &[
    ("AGENTS.md", include_str!("../assets/agent/AGENTS.md")),
    ("CLAUDE.md", include_str!("../assets/agent/CLAUDE.md")),
    (
        ".claude/skills/assess/SKILL.md",
        include_str!("../assets/agent/.claude/skills/assess/SKILL.md"),
    ),
    (
        ".claude/skills/feedback-compact/SKILL.md",
        include_str!("../assets/agent/.claude/skills/feedback-compact/SKILL.md"),
    ),
];

/// A workspace file compared against its embedded default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Default,
    Edited,
    Missing,
}

pub struct AgentWorkspace {
    pub dir: PathBuf,
}

impl AgentWorkspace {
    /// The directory the agent runs in: the `agent.workspace_dir` override or
    /// the default next to the config file.
    pub fn resolve_dir(agent: &AgentConfig, paths: &Paths) -> PathBuf {
        match &agent.workspace_dir {
            Some(dir) => expand_tilde(dir),
            None => paths.agent_workspace_dir.clone(),
        }
    }

    /// Open a workspace without touching the filesystem (for read-only
    /// inspection, e.g. `config check`).
    pub fn open(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Open the workspace, writing any missing file from the embedded
    /// defaults. Existing (possibly user-edited) files are never touched.
    /// Returns the workspace plus the files it created.
    pub fn ensure(dir: PathBuf) -> Result<(Self, Vec<&'static str>)> {
        let mut created = Vec::new();
        for (rel, content) in DEFAULT_FILES {
            let path = dir.join(rel);
            if path.exists() {
                continue;
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("cannot create {}", parent.display()))?;
            }
            std::fs::write(&path, content)
                .with_context(|| format!("cannot write {}", path.display()))?;
            created.push(*rel);
        }
        Ok((Self { dir }, created))
    }

    fn read(&self, rel: &str) -> Result<String> {
        let path = self.dir.join(rel);
        std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read workspace file {}", path.display()))
    }

    /// 16-hex digest of the instructions in effect for one task: AGENTS.md
    /// plus that task's SKILL.md, as found on disk (user edits included).
    /// Editing either invalidates assessments made under the old text;
    /// editing an unrelated task's skill or CLAUDE.md does not.
    pub fn instructions_hash(&self, task: AgentTask) -> Result<String> {
        let agents = self.read("AGENTS.md")?;
        let skill_rel = task.skill_path();
        let skill = self.read(&skill_rel)?;
        let preimage = format!("AGENTS.md\0{agents}\0{skill_rel}\0{skill}");
        Ok(crate::sha256_hex(preimage.as_bytes())[..16].to_string())
    }

    /// Per-file drift against the embedded defaults. Read-only.
    pub fn file_status(&self) -> Vec<(&'static str, FileStatus)> {
        DEFAULT_FILES
            .iter()
            .map(|(rel, default)| {
                let status = match std::fs::read_to_string(self.dir.join(rel)) {
                    Ok(text) if text == *default => FileStatus::Default,
                    Ok(_) => FileStatus::Edited,
                    Err(_) => FileStatus::Missing,
                };
                (*rel, status)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn embedded(rel: &str) -> &'static str {
        DEFAULT_FILES.iter().find(|(r, _)| *r == rel).unwrap().1
    }

    #[test]
    fn materializes_missing_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let (ws, created) = AgentWorkspace::ensure(dir.path().to_path_buf()).unwrap();
        assert_eq!(created.len(), DEFAULT_FILES.len());
        assert!(ws.file_status().iter().all(|(_, s)| *s == FileStatus::Default));

        // Hand-edit one file; ensure again must not touch it or re-create others.
        let assess = dir.path().join(AgentTask::Assess.skill_path());
        std::fs::write(&assess, "my own rules").unwrap();
        let (ws, created) = AgentWorkspace::ensure(dir.path().to_path_buf()).unwrap();
        assert!(created.is_empty());
        assert_eq!(std::fs::read_to_string(&assess).unwrap(), "my own rules");
        let statuses = ws.file_status();
        let assess_status =
            statuses.iter().find(|(r, _)| *r == ".claude/skills/assess/SKILL.md").unwrap();
        assert_eq!(assess_status.1, FileStatus::Edited);
    }

    #[test]
    fn hash_tracks_agents_md_and_own_skill_only() {
        let dir = tempfile::tempdir().unwrap();
        let (ws, _) = AgentWorkspace::ensure(dir.path().to_path_buf()).unwrap();
        let assess = ws.instructions_hash(AgentTask::Assess).unwrap();
        let compact = ws.instructions_hash(AgentTask::FeedbackCompact).unwrap();
        assert_eq!(assess.len(), 16);

        // Editing an unrelated skill or CLAUDE.md leaves the assess hash alone.
        std::fs::write(dir.path().join(AgentTask::FeedbackCompact.skill_path()), "x").unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "something else").unwrap();
        assert_eq!(ws.instructions_hash(AgentTask::Assess).unwrap(), assess);
        assert_ne!(ws.instructions_hash(AgentTask::FeedbackCompact).unwrap(), compact);

        // Editing the task's own skill or AGENTS.md changes it.
        std::fs::write(dir.path().join(AgentTask::Assess.skill_path()), "new rules").unwrap();
        let after_skill = ws.instructions_hash(AgentTask::Assess).unwrap();
        assert_ne!(after_skill, assess);
        std::fs::write(dir.path().join("AGENTS.md"), "new conventions").unwrap();
        assert_ne!(ws.instructions_hash(AgentTask::Assess).unwrap(), after_skill);
    }

    #[test]
    fn pointer_names_the_skill_and_gives_its_path() {
        // One line, two vocabularies: skill-aware agents match the name they
        // discovered in the workspace, plain argv agents read the path.
        let pointer = AgentTask::FeedbackCompact.instructions_pointer();
        assert!(pointer.contains("`feedback-compact` skill"));
        assert!(pointer.contains(".claude/skills/feedback-compact/SKILL.md"));
        assert!(pointer.contains("AGENTS.md"));
        assert!(!pointer.contains('\n'), "the pointer is one prompt line");
    }

    #[test]
    fn hash_is_deterministic_across_pristine_workspaces() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (wa, _) = AgentWorkspace::ensure(a.path().to_path_buf()).unwrap();
        let (wb, _) = AgentWorkspace::ensure(b.path().to_path_buf()).unwrap();
        assert_eq!(
            wa.instructions_hash(AgentTask::Assess).unwrap(),
            wb.instructions_hash(AgentTask::Assess).unwrap()
        );
    }

    #[test]
    fn assets_dir_matches_embedded_list() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/agent");
        let mut on_disk = Vec::new();
        let mut stack = vec![assets.clone()];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let rel = path.strip_prefix(&assets).unwrap();
                    on_disk.push(rel.to_string_lossy().into_owned());
                }
            }
        }
        let mut embedded: Vec<String> =
            DEFAULT_FILES.iter().map(|(r, _)| r.to_string()).collect();
        on_disk.sort();
        embedded.sort();
        assert_eq!(on_disk, embedded, "assets/agent and DEFAULT_FILES are out of sync");
    }

    #[test]
    fn shipped_defaults_carry_the_load_bearing_rules() {
        // These phrases are behavioral contracts; removing them from the
        // shipped skill is a deliberate act, not an editing accident.
        let assess = embedded(".claude/skills/assess/SKILL.md");
        assert!(assess.contains("estimate a realistic market range"));
        assert!(assess.contains("is_estimate=true"));
        assert!(assess.contains("never collapse a stated range into one number"));
        assert!(assess.contains("fit.suggestions"));

        // The assess skill guards the layer split: preferences may not move
        // any scored field.
        assert!(assess.contains("is preference, not evidence"));

        let compact = embedded(".claude/skills/feedback-compact/SKILL.md");
        assert!(compact.contains("AT MOST 10"));

        assert_eq!(embedded("CLAUDE.md").trim(), "@AGENTS.md");
        for task in [AgentTask::Assess, AgentTask::FeedbackCompact] {
            assert!(embedded("AGENTS.md").contains(&task.skill_path()));
        }
    }
}
