//! API token for the ingest server: generated on first use, stored 0600,
//! pasted once into the browser extension. A mitigation against junk posts
//! from arbitrary local processes/webpages, not a security boundary.

use anyhow::{Context, Result};
use rand::RngExt;
use std::path::Path;

pub fn load_or_create(path: &Path) -> Result<String> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        let token = existing.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    let token = hex::encode(bytes);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    write_private(path, &token)?;
    Ok(token)
}

pub fn load(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let token = text.trim().to_string();
            Ok((!token.is_empty()).then_some(token))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

#[cfg(unix)]
fn write_private(path: &Path, token: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    file.write_all(token.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &Path, token: &str) -> Result<()> {
    std::fs::write(path, format!("{token}\n"))
        .with_context(|| format!("cannot write {}", path.display()))
}

/// Constant-time comparison so the check leaks nothing via timing.
pub fn verify(expected: &str, provided: &str) -> bool {
    let (a, b) = (expected.as_bytes(), provided.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_then_reuses_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api_token");
        let first = load_or_create(&path).unwrap();
        assert_eq!(first.len(), 64);
        let second = load_or_create(&path).unwrap();
        assert_eq!(first, second);
        assert_eq!(load(&path).unwrap(), Some(first));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn verify_is_exact() {
        assert!(verify("abc", "abc"));
        assert!(!verify("abc", "abd"));
        assert!(!verify("abc", "abcd"));
    }

    #[test]
    fn load_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(&dir.path().join("nope")).unwrap(), None);
    }
}
