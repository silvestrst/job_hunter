//! The candidate profile: the CV plus two optional supplementary documents -
//! deeper detail on experience and skills, and what the candidate wants from a
//! role. Loaded once per command, hashed so an edit invalidates cached work,
//! and rendered into prompts through one shared set of section headings.
//!
//! The CV keeps its own `cv_hash` (and its own digest cache); the supplements
//! hash separately, so adding one re-runs assessments without changing what a
//! CV edit means.

use crate::assessment::prompt::truncate_chars;
use crate::config::ResourcesConfig;
use crate::paths::expand_tilde;
use anyhow::{Context, Result};

/// The profile as it sits on disk.
#[derive(Debug, Clone)]
pub struct Profile {
    pub cv_text: String,
    /// sha256 of the CV file - the same value this tool has always stored, so
    /// introducing the supplements left every existing assessment valid.
    pub cv_hash: String,
    pub background: Option<String>,
    pub preferences: Option<String>,
}

impl Profile {
    /// Read every configured document. The CV is required; a supplement that
    /// is configured but unreadable is an error rather than a silent omission,
    /// because a prompt quietly missing half the profile is worse than a stop.
    pub fn load(resources: &ResourcesConfig) -> Result<Self> {
        let cv_path = expand_tilde(&resources.cv_path);
        let cv_text = std::fs::read_to_string(&cv_path).with_context(|| {
            format!("cannot read CV at {} (resources.cv_path)", cv_path.display())
        })?;
        Ok(Self {
            cv_hash: crate::sha256_hex(cv_text.as_bytes()),
            cv_text,
            background: read_optional(resources.background_path.as_deref(), "background_path")?,
            preferences: read_optional(resources.preferences_path.as_deref(), "preferences_path")?,
        })
    }

    /// 16-hex digest of the supplements, or None when there are none. None is
    /// stored as SQL NULL and compared with `IS`, so rows written before the
    /// supplements existed match a profile that still has none - and stop
    /// matching the moment one is added.
    pub fn supplement_hash(&self) -> Option<String> {
        if self.background.is_none() && self.preferences.is_none() {
            return None;
        }
        let preimage = format!(
            "background\0{}\0preferences\0{}",
            self.background.as_deref().unwrap_or(""),
            self.preferences.as_deref().unwrap_or("")
        );
        Some(crate::sha256_hex(preimage.as_bytes())[..16].to_string())
    }

    /// Pair the profile with the CV text a prompt should actually carry -
    /// verbatim for `cv-advice`, possibly the cached digest for `assess` -
    /// trimming the supplements to their budget.
    pub fn for_prompt(&self, cv_text: String, max_chars: usize) -> ProfilePrompt {
        let trim = |doc: &Option<String>| doc.as_deref().map(|d| truncate_chars(d, max_chars));
        ProfilePrompt {
            cv_text,
            background: trim(&self.background),
            preferences: trim(&self.preferences),
        }
    }
}

/// The profile as it enters a prompt: CV text plus budget-trimmed supplements.
#[derive(Debug, Clone)]
pub struct ProfilePrompt {
    pub cv_text: String,
    pub background: Option<String>,
    pub preferences: Option<String>,
}

impl ProfilePrompt {
    /// The supplement sections that follow the CV in every prompt carrying the
    /// profile. Empty when nothing is configured; the headings live here so
    /// `assess` and `cv-advice` name the documents identically - the skills
    /// refer to these headings by name.
    pub fn sections(&self) -> String {
        let mut out = String::new();
        if let Some(background) = &self.background {
            out.push_str(&format!("\n## Additional background (detail beyond the CV)\n{background}\n"));
        }
        if let Some(preferences) = &self.preferences {
            out.push_str(&format!("\n## What the candidate wants from a role\n{preferences}\n"));
        }
        out
    }
}

/// A configured supplementary document. Whitespace-only counts as absent, so
/// an empty placeholder file doesn't put a hollow heading in every prompt.
fn read_optional(path: Option<&str>, key: &str) -> Result<Option<String>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let expanded = expand_tilde(path);
    let text = std::fs::read_to_string(&expanded)
        .with_context(|| format!("cannot read {} (resources.{key})", expanded.display()))?;
    Ok((!text.trim().is_empty()).then(|| text.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn resources(dir: &Path, background: Option<&str>, preferences: Option<&str>) -> ResourcesConfig {
        std::fs::write(dir.join("cv.md"), "MY CV").unwrap();
        ResourcesConfig {
            cv_path: dir.join("cv.md").to_string_lossy().into_owned(),
            background_path: background.map(|name| dir.join(name).to_string_lossy().into_owned()),
            preferences_path: preferences.map(|name| dir.join(name).to_string_lossy().into_owned()),
        }
    }

    #[test]
    fn loads_cv_alone_with_no_supplement_hash() {
        let dir = tempfile::tempdir().unwrap();
        let profile = Profile::load(&resources(dir.path(), None, None)).unwrap();
        assert_eq!(profile.cv_text, "MY CV");
        assert_eq!(profile.cv_hash, crate::sha256_hex(b"MY CV"));
        assert!(profile.supplement_hash().is_none(), "no supplements = nothing to invalidate");
        assert_eq!(profile.for_prompt("MY CV".into(), 100).sections(), "");
    }

    #[test]
    fn supplements_are_loaded_hashed_and_sectioned() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bg.md"), "  led the payments rewrite\n").unwrap();
        std::fs::write(dir.path().join("want.md"), "remote, no on-call").unwrap();
        let profile =
            Profile::load(&resources(dir.path(), Some("bg.md"), Some("want.md"))).unwrap();
        assert_eq!(profile.background.as_deref(), Some("led the payments rewrite"));

        let first = profile.supplement_hash().unwrap();
        assert_eq!(first.len(), 16);

        // A supplement edit changes the hash; the CV hash is untouched by it.
        std::fs::write(dir.path().join("want.md"), "remote only").unwrap();
        let edited = Profile::load(&resources(dir.path(), Some("bg.md"), Some("want.md"))).unwrap();
        assert_ne!(edited.supplement_hash().unwrap(), first);
        assert_eq!(edited.cv_hash, profile.cv_hash);

        let sections = profile.for_prompt("CV".into(), 100).sections();
        assert!(sections.contains("## Additional background"));
        assert!(sections.contains("led the payments rewrite"));
        assert!(sections.contains("## What the candidate wants from a role"));
        assert!(sections.contains("no on-call"));
    }

    #[test]
    fn blank_supplement_reads_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bg.md"), "   \n\n").unwrap();
        let profile = Profile::load(&resources(dir.path(), Some("bg.md"), None)).unwrap();
        assert!(profile.background.is_none());
        assert!(profile.supplement_hash().is_none());
    }

    #[test]
    fn configured_but_missing_supplement_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = Profile::load(&resources(dir.path(), Some("absent.md"), None)).unwrap_err();
        assert!(format!("{err:#}").contains("resources.background_path"));
    }

    #[test]
    fn supplements_are_trimmed_to_budget() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bg.md"), "x".repeat(50)).unwrap();
        let profile = Profile::load(&resources(dir.path(), Some("bg.md"), None)).unwrap();
        let sections = profile.for_prompt("CV".into(), 10).sections();
        assert!(sections.contains("truncated"));
    }
}
