//! The candidate profile: one experience document, plus an optional statement
//! of what the candidate wants from a role. Loaded once per command, hashed so
//! an edit invalidates cached work, and rendered into prompts through one
//! shared set of section headings.
//!
//! Every configured file is size-checked before it is opened. That guard is
//! the only limit on the way in - nothing is summarised or truncated
//! afterwards - so what a prompt carries is exactly what is on disk.

use crate::config::ResourcesConfig;
use crate::paths::expand_tilde;
use anyhow::{bail, Context, Result};
use std::path::Path;

/// The profile as it sits on disk.
#[derive(Debug, Clone)]
pub struct Profile {
    pub experience: String,
    /// sha256 of the experience file: the assessment cache key, so any edit
    /// re-runs every job against the new evidence.
    pub experience_hash: String,
    pub preferences: Option<String>,
}

impl Profile {
    /// Read every configured document. The experience file is required; a
    /// preferences file that is configured but unreadable is an error rather
    /// than a silent omission, because a prompt quietly missing half the
    /// profile is worse than a stop.
    pub fn load(resources: &ResourcesConfig) -> Result<Self> {
        let limit = resources.max_file_bytes;
        let experience = read_input_file(
            &expand_tilde(&resources.experience_path),
            "resources.experience_path",
            limit,
        )?;
        // Whitespace-only counts as absent, so an empty placeholder file
        // doesn't put a hollow heading in every prompt.
        let preferences = match &resources.preferences_path {
            None => None,
            Some(path) => {
                let text =
                    read_input_file(&expand_tilde(path), "resources.preferences_path", limit)?;
                (!text.trim().is_empty()).then(|| text.trim().to_string())
            }
        };
        Ok(Self {
            experience_hash: crate::sha256_hex(experience.as_bytes()),
            experience,
            preferences,
        })
    }

    /// 16-hex digest of the preferences, or None when none are configured.
    /// None is stored as SQL NULL and compared with `IS`, so a profile that
    /// never had preferences keeps matching its stored assessments - and stops
    /// matching the moment preferences are added.
    pub fn preferences_hash(&self) -> Option<String> {
        self.preferences
            .as_deref()
            .map(|p| crate::sha256_hex(p.as_bytes())[..16].to_string())
    }

    /// The profile as it enters a prompt. The headings live here so every
    /// prompt names the documents identically - the skills refer to them by
    /// name, and the split between evidence and preference is load-bearing:
    /// the first section is fact, the second may only colour prose.
    pub fn sections(&self) -> String {
        let experience = &self.experience;
        let mut out = format!("## Candidate experience\n{experience}\n");
        if let Some(preferences) = &self.preferences {
            out.push_str(&format!(
                "\n## What the candidate wants from a role\n{preferences}\n"
            ));
        }
        out
    }
}

/// Read one configured input file, refusing anything over the size guard
/// before opening it. The usual cause of an oversized file is a path pointing
/// at the wrong thing, so the error reports both numbers and both remedies -
/// that is what makes a mistyped path obvious rather than expensive.
fn read_input_file(path: &Path, key: &str, max_bytes: u64) -> Result<String> {
    let meta = std::fs::metadata(path)
        .with_context(|| format!("cannot read {} ({key})", path.display()))?;
    if !meta.is_file() {
        bail!("{} ({key}) is not a file", path.display());
    }
    if meta.len() > max_bytes {
        bail!(
            "{} ({key}) is {} bytes, over the {max_bytes}-byte limit - check the path \
             points at the file you meant, or raise resources.max_file_bytes",
            path.display(),
            meta.len()
        );
    }
    std::fs::read_to_string(path)
        .with_context(|| format!("cannot read {} ({key})", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resources(dir: &Path, preferences: Option<&str>) -> ResourcesConfig {
        std::fs::write(dir.join("experience.md"), "MY EXPERIENCE").unwrap();
        ResourcesConfig {
            experience_path: dir.join("experience.md").to_string_lossy().into_owned(),
            preferences_path: preferences.map(|n| dir.join(n).to_string_lossy().into_owned()),
            max_file_bytes: 8_192,
        }
    }

    #[test]
    fn loads_experience_alone_with_no_preferences_hash() {
        let dir = tempfile::tempdir().unwrap();
        let profile = Profile::load(&resources(dir.path(), None)).unwrap();
        assert_eq!(profile.experience, "MY EXPERIENCE");
        assert_eq!(profile.experience_hash, crate::sha256_hex(b"MY EXPERIENCE"));
        assert!(profile.preferences_hash().is_none(), "nothing to invalidate");

        let sections = profile.sections();
        assert!(sections.contains("## Candidate experience"));
        assert!(sections.contains("MY EXPERIENCE"));
        assert!(!sections.contains("wants from a role"));
    }

    #[test]
    fn preferences_are_loaded_hashed_and_sectioned() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("want.md"), "  remote, no on-call\n").unwrap();
        let profile = Profile::load(&resources(dir.path(), Some("want.md"))).unwrap();
        assert_eq!(profile.preferences.as_deref(), Some("remote, no on-call"));

        let first = profile.preferences_hash().unwrap();
        assert_eq!(first.len(), 16);

        // A preferences edit changes its hash; the experience hash is untouched.
        std::fs::write(dir.path().join("want.md"), "remote only").unwrap();
        let edited = Profile::load(&resources(dir.path(), Some("want.md"))).unwrap();
        assert_ne!(edited.preferences_hash().unwrap(), first);
        assert_eq!(edited.experience_hash, profile.experience_hash);

        let sections = profile.sections();
        assert!(sections.contains("## What the candidate wants from a role"));
        assert!(sections.contains("no on-call"));
    }

    #[test]
    fn blank_preferences_read_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("want.md"), "   \n\n").unwrap();
        let profile = Profile::load(&resources(dir.path(), Some("want.md"))).unwrap();
        assert!(profile.preferences.is_none());
        assert!(profile.preferences_hash().is_none());
    }

    #[test]
    fn configured_but_missing_preferences_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = Profile::load(&resources(dir.path(), Some("absent.md"))).unwrap_err();
        assert!(format!("{err:#}").contains("resources.preferences_path"));
    }

    #[test]
    fn oversized_experience_file_is_refused_unread() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = resources(dir.path(), None);
        std::fs::write(dir.path().join("experience.md"), "x".repeat(9_000)).unwrap();
        let err = Profile::load(&config).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("9000 bytes"), "{msg}");
        assert!(msg.contains("8192-byte limit"), "{msg}");
        assert!(msg.contains("resources.max_file_bytes"), "{msg}");

        // The limit is the only thing standing between the file and a prompt,
        // so raising it must be all it takes to let the same file through.
        config.max_file_bytes = 9_000;
        assert_eq!(Profile::load(&config).unwrap().experience.len(), 9_000);
    }

    #[test]
    fn oversized_preferences_file_is_refused_too() {
        // The guard is per input file, not just the required one.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("want.md"), "x".repeat(9_000)).unwrap();
        let err = Profile::load(&resources(dir.path(), Some("want.md"))).unwrap_err();
        assert!(format!("{err:#}").contains("resources.preferences_path"));
    }

    #[test]
    fn a_directory_is_not_an_input_file() {
        // Directories have a size on disk too; without this check a path
        // pointing at one would fail later and less clearly.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("notes")).unwrap();
        let mut config = resources(dir.path(), None);
        config.experience_path = dir.path().join("notes").to_string_lossy().into_owned();
        assert!(format!("{:#}", Profile::load(&config).unwrap_err()).contains("is not a file"));
    }
}
