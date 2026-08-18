pub mod agent;
pub mod assessment;
pub mod cleaner;
pub mod config;
pub mod db;
pub mod feedback;
pub mod ingest;
pub mod paths;
pub mod scoring;
pub mod token;
pub mod urlnorm;

/// ISO-8601 UTC timestamp used for every created_at/updated_at column.
pub fn now_utc() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Canonical form of a job title / company name used for the title+company
/// dedupe layer: lowercase, punctuation to spaces, whitespace collapsed.
pub fn normalize_identity(s: &str) -> Option<String> {
    let cleaned: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then_some(collapsed)
}

#[cfg(test)]
mod tests {
    #[test]
    fn identity_normalization() {
        assert_eq!(
            super::normalize_identity("  Senior Rust Engineer - (Remote)!"),
            Some("senior rust engineer remote".to_string())
        );
        assert_eq!(super::normalize_identity("Acme, Ltd."), Some("acme ltd".to_string()));
        assert_eq!(super::normalize_identity("  ??? "), None);
    }
}
