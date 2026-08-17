use anyhow::{Context, Result};
use url::Url;

/// Query parameters that carry tracking state, not identity.
const TRACKING_PARAMS: &[&str] = &[
    "gclid", "fbclid", "msclkid", "ref", "refid", "trk", "trackingid", "source", "src",
    "campaign", "mc_cid", "mc_eid", "igshid",
];

fn is_tracking_param(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("utm_") || TRACKING_PARAMS.contains(&lower.as_str())
}

/// Canonical form used as the URL-level dedupe key: lowercase scheme/host,
/// no fragment, no tracking params, remaining query params sorted, no
/// trailing slash.
pub fn normalize(raw: &str) -> Result<String> {
    let mut url = Url::parse(raw.trim()).with_context(|| format!("invalid URL: {raw}"))?;
    url.set_fragment(None);

    let mut params: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !is_tracking_param(k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    params.sort();
    if params.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(&params);
    }

    let mut out = url.to_string();
    if out.ends_with('/') && url.path() == "/" && url.query().is_none() {
        out.pop();
    } else if url.query().is_none() && url.path().ends_with('/') && url.path() != "/" {
        out.pop();
    }
    Ok(out)
}

pub fn domain(raw: &str) -> Result<String> {
    let url = Url::parse(raw.trim()).with_context(|| format!("invalid URL: {raw}"))?;
    Ok(url.host_str().unwrap_or_default().to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tracking_and_fragment() {
        let n = normalize(
            "https://Example.com/jobs/123?utm_source=li&b=2&a=1&gclid=xyz#apply",
        )
        .unwrap();
        assert_eq!(n, "https://example.com/jobs/123?a=1&b=2");
    }

    #[test]
    fn equivalent_urls_normalize_identically() {
        let a = normalize("https://example.com/jobs/123/").unwrap();
        let b = normalize("https://EXAMPLE.com/jobs/123?utm_campaign=x").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn bare_host_loses_trailing_slash() {
        assert_eq!(normalize("https://example.com/").unwrap(), "https://example.com");
    }

    #[test]
    fn domain_extraction() {
        assert_eq!(domain("https://Jobs.Example.co.uk/x?y=1").unwrap(), "jobs.example.co.uk");
    }
}
