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

    /// The JSON contract this task's prompt carries, when it has one. Part of
    /// the instructions hash - see `AgentWorkspace::instructions_hash`.
    pub fn contract(&self) -> Option<&'static str> {
        match self {
            AgentTask::Assess => Some(crate::assessment::prompt::FACTS_CONTRACT),
            AgentTask::FeedbackCompact => None,
        }
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

/// One workspace file: where it lives, what ships in the binary, and the
/// sha256 of every default this binary supersedes.
struct DefaultFile {
    rel: &'static str,
    content: &'static str,
    /// Defaults shipped by earlier binaries. A file matching one of these was
    /// written by job-hunter and never touched since, so replacing it restores
    /// nothing the user wrote - it is an upgrade, not an overwrite. **Append
    /// the outgoing default's hash here whenever `content` changes**, or the
    /// old text silently stays in place on every existing install.
    superseded: &'static [&'static str],
}

/// Every workspace file. The assets directory mirrors the workspace layout
/// exactly; a test asserts the two stay in sync so a new asset can't silently
/// fail to ship.
const DEFAULT_FILES: &[DefaultFile] = &[
    DefaultFile {
        rel: "AGENTS.md",
        content: include_str!("../assets/agent/AGENTS.md"),
        superseded: &[],
    },
    DefaultFile {
        rel: "CLAUDE.md",
        content: include_str!("../assets/agent/CLAUDE.md"),
        superseded: &[],
    },
    DefaultFile {
        rel: ".claude/skills/assess/SKILL.md",
        content: include_str!("../assets/agent/.claude/skills/assess/SKILL.md"),
        // Pre-2026-08-24: scoring was weighted criteria and preferences had no
        // grade of their own.
        superseded: &["a63d128f4e88fe5e0148a5f9be3a1b08fae6027d232a700fee6e8058b9ac9d74"],
    },
    DefaultFile {
        rel: ".claude/skills/feedback-compact/SKILL.md",
        content: include_str!("../assets/agent/.claude/skills/feedback-compact/SKILL.md"),
        superseded: &[],
    },
];

/// A workspace file compared against its embedded default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Default,
    /// Matches a default an earlier binary shipped: nobody has edited it, so
    /// the next `ensure` upgrades it in place.
    Outdated,
    Edited,
    Missing,
}

/// Where one on-disk file stands against what this binary ships. Anything that
/// is neither the current default nor a default we once shipped is the user's,
/// including text we simply don't recognise - the safe reading, since the cost
/// of guessing wrong is destroying someone's instructions.
fn classify(on_disk: Option<&str>, content: &str, superseded: &[&str]) -> FileStatus {
    let Some(text) = on_disk else {
        return FileStatus::Missing;
    };
    if text == content {
        return FileStatus::Default;
    }
    if superseded.contains(&crate::sha256_hex(text.as_bytes()).as_str()) {
        return FileStatus::Outdated;
    }
    FileStatus::Edited
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

    /// Open the workspace, writing any file that is missing or still carries a
    /// default an older binary shipped. Edited files are never touched -
    /// that is the whole contract of this directory - but a file nobody has
    /// edited is the binary's to keep current, and leaving it stale would mean
    /// new instructions never reach an existing install.
    /// Returns the workspace plus the files it wrote.
    pub fn ensure(dir: PathBuf) -> Result<(Self, Vec<&'static str>)> {
        let mut written = Vec::new();
        let workspace = Self { dir };
        for (rel, status) in workspace.file_status() {
            if !matches!(status, FileStatus::Missing | FileStatus::Outdated) {
                continue;
            }
            let file = DEFAULT_FILES.iter().find(|f| f.rel == rel).expect("rel comes from the list");
            let path = workspace.dir.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("cannot create {}", parent.display()))?;
            }
            std::fs::write(&path, file.content)
                .with_context(|| format!("cannot write {}", path.display()))?;
            written.push(rel);
        }
        Ok((workspace, written))
    }

    fn read(&self, rel: &str) -> Result<String> {
        let path = self.dir.join(rel);
        std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read workspace file {}", path.display()))
    }

    /// 16-hex digest of everything that tells the agent what to produce for
    /// one task: AGENTS.md and that task's SKILL.md as found on disk (user
    /// edits included), plus the JSON contract the task's prompt carries.
    /// Any of them changing invalidates assessments made under the old text;
    /// editing an unrelated task's skill or CLAUDE.md does not. The contract
    /// belongs in here because it lives in the binary rather than the
    /// workspace, and without it a schema change would leave every stored
    /// assessment looking current while missing the new fields.
    pub fn instructions_hash(&self, task: AgentTask) -> Result<String> {
        let agents = self.read("AGENTS.md")?;
        let skill_rel = task.skill_path();
        let skill = self.read(&skill_rel)?;
        let contract = task.contract().unwrap_or("");
        let preimage = format!("AGENTS.md\0{agents}\0{skill_rel}\0{skill}\0contract\0{contract}");
        Ok(crate::sha256_hex(preimage.as_bytes())[..16].to_string())
    }

    /// Per-file drift against the embedded defaults. Read-only.
    pub fn file_status(&self) -> Vec<(&'static str, FileStatus)> {
        DEFAULT_FILES
            .iter()
            .map(|file| {
                let on_disk = std::fs::read_to_string(self.dir.join(file.rel)).ok();
                (file.rel, classify(on_disk.as_deref(), file.content, file.superseded))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn embedded(rel: &str) -> &'static str {
        DEFAULT_FILES.iter().find(|f| f.rel == rel).unwrap().content
    }

    fn status_of(ws: &AgentWorkspace, rel: &str) -> FileStatus {
        ws.file_status().into_iter().find(|(r, _)| *r == rel).unwrap().1
    }

    #[test]
    fn materializes_missing_files_and_leaves_edits_alone() {
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
        assert_eq!(status_of(&ws, ".claude/skills/assess/SKILL.md"), FileStatus::Edited);
    }

    #[test]
    fn superseded_defaults_are_upgradeable_but_edits_are_not() {
        // The file nobody edited is the binary's to keep current: without
        // Outdated, an existing install keeps running last release's
        // instructions forever, because ensure() only wrote missing files.
        // Anything unrecognised must fall to Edited - guessing wrong here
        // destroys someone's instructions.
        let current = "the current default\n";
        let old = "the default an older binary shipped\n";
        let old_hash = crate::sha256_hex(old.as_bytes());
        let superseded = [old_hash.as_str()];

        assert_eq!(classify(None, current, &superseded), FileStatus::Missing);
        assert_eq!(classify(Some(current), current, &superseded), FileStatus::Default);
        assert_eq!(classify(Some(old), current, &superseded), FileStatus::Outdated);
        assert_eq!(classify(Some("my own rules"), current, &superseded), FileStatus::Edited);
    }

    #[test]
    fn the_assess_skill_records_the_default_it_replaced() {
        // The one step that is easy to forget when editing a shipped skill,
        // and silent when forgotten.
        let file = DEFAULT_FILES
            .iter()
            .find(|f| f.rel == ".claude/skills/assess/SKILL.md")
            .unwrap();
        assert!(!file.superseded.is_empty(), "append the outgoing default's sha256");
        for hash in file.superseded {
            assert_eq!(hash.len(), 64, "superseded entries are full sha256 hex: {hash}");
            assert!(hash.chars().all(|c| c.is_ascii_hexdigit()), "not hex: {hash}");
            assert_ne!(
                *hash,
                crate::sha256_hex(file.content.as_bytes()),
                "the current default cannot also be a superseded one"
            );
        }
    }

    #[test]
    fn instructions_hash_covers_the_json_contract() {
        // The contract lives in the binary, not the workspace. If it were left
        // out, adding a field to the facts schema would leave every stored
        // assessment looking current while missing that field.
        let dir = tempfile::tempdir().unwrap();
        let (ws, _) = AgentWorkspace::ensure(dir.path().to_path_buf()).unwrap();
        let agents = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        let skill_rel = AgentTask::Assess.skill_path();
        let skill = std::fs::read_to_string(dir.path().join(&skill_rel)).unwrap();

        let without_contract =
            crate::sha256_hex(format!("AGENTS.md\0{agents}\0{skill_rel}\0{skill}").as_bytes())
                [..16]
                .to_string();
        assert_ne!(ws.instructions_hash(AgentTask::Assess).unwrap(), without_contract);
        assert!(AgentTask::Assess.contract().unwrap().contains("preference_match"));
        assert!(AgentTask::FeedbackCompact.contract().is_none());
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
            DEFAULT_FILES.iter().map(|f| f.rel.to_string()).collect();
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
        // any scored field, and produce a grade of their own instead.
        assert!(assess.contains("is preference, not evidence"));
        assert!(assess.contains("poor | average | good | strong |"));
        assert!(assess.contains("the score is their mean"));

        let compact = embedded(".claude/skills/feedback-compact/SKILL.md");
        assert!(compact.contains("AT MOST 10"));

        assert_eq!(embedded("CLAUDE.md").trim(), "@AGENTS.md");
        for task in [AgentTask::Assess, AgentTask::FeedbackCompact] {
            assert!(embedded("AGENTS.md").contains(&task.skill_path()));
        }
    }
}
