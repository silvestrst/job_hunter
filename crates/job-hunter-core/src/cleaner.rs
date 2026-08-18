//! Domain-specific cleanup of scraped page text, applied at ingest. Job
//! boards wrap postings in heavy chrome (nav bars, promo modules, footers);
//! where that noise sits behind stable text markers we trim to just the
//! posting. Conservative by design: when the markers aren't found, or the
//! trim leaves suspiciously little behind, the original text is kept.

/// Returns cleaned text, or None when no rule applies or cleaning was
/// judged unsafe (caller stores the original in that case).
pub fn clean(domain: &str, text: &str) -> Option<String> {
    if domain == "linkedin.com" || domain.ends_with(".linkedin.com") {
        return clean_linkedin(text);
    }
    None
}

/// Everything from these lines onward is page chrome: similar-job promos,
/// "insights", company modules, footer, language picker.
const LINKEDIN_END_MARKERS: &[&str] = &[
    "Set alert for similar jobs",
    "See how you compare to others who clicked apply",
    "Exclusive Job Seeker Insights",
    "About the company",
    "More jobs",
    "Looking for talent?",
];

/// Action buttons and AI-helper chrome that sit between the job header and
/// "About the job". Matched as whole lines; the useful chips in the same
/// region (Remote / Full-time / salary) don't collide with any of these.
const LINKEDIN_HEADER_JUNK: &[&str] = &[
    "Apply",
    "Easy Apply",
    "Save",
    "Saved",
    "Message",
    "Responses managed off LinkedIn",
    "Use AI to assess how you fit",
    "Show match details",
    "Tailor my resume",
    "Create cover letter",
    "Help me stand out",
    "People you can reach out to",
    "People in your network",
];

/// A LinkedIn job view renders as: global nav, then a header block
/// (company / title / "location · N weeks ago · M people clicked apply"
/// plus Remote/Full-time/salary chips), then "About the job" with the
/// actual description, then endless promos and footer. Keep header block
/// through description, drop the rest.
fn clean_linkedin(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let about = lines.iter().position(|l| l.trim() == "About the job")?;

    // Header anchor: the "location · 3 weeks ago · 13 people clicked apply"
    // line. The title and company are the two non-empty lines above it.
    let start = match lines[..about]
        .iter()
        .rposition(|l| l.contains(" · ") && l.contains("ago"))
    {
        Some(anchor) => {
            let mut start = anchor;
            let mut kept = 0;
            for (i, line) in lines[..anchor].iter().enumerate().rev() {
                if line.trim().is_empty() {
                    continue;
                }
                start = i;
                kept += 1;
                if kept == 2 {
                    break;
                }
            }
            start
        }
        None => about,
    };

    let end = lines[about..]
        .iter()
        .position(|l| LINKEDIN_END_MARKERS.contains(&l.trim()))
        .map(|i| about + i)
        .unwrap_or(lines.len());

    let mut out = String::new();
    let mut blank_pending = false;
    let mut in_people_module = false;
    for (i, line) in lines[start..end].iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_pending = !out.is_empty();
            continue;
        }
        // Junk only appears above the description; a posting's own bullet
        // could legitimately be e.g. "Apply" below it.
        if start + i < about {
            // "People you can reach out to" opens a connections module
            // (names + headlines, not enumerable) that runs up to the
            // description - drop it wholesale.
            if trimmed == "People you can reach out to" {
                in_people_module = true;
            }
            if in_people_module || LINKEDIN_HEADER_JUNK.contains(&trimmed) {
                continue;
            }
        }
        if blank_pending {
            out.push('\n');
            blank_pending = false;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(trimmed);
    }

    // Drop a final line that is just LinkedIn's "… more" expander stub.
    let out = match out.rsplit_once('\n') {
        Some((rest, last)) if matches!(last.trim(), "… more" | "…more" | "... more" | "...more") => {
            rest.trim_end().to_string()
        }
        _ => out,
    };

    // A real posting survived the cut, or we keep the original.
    (out.chars().count() >= 200).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILLER: &str = "The role involves building low-latency trading systems in Rust, \
        owning services end to end, and collaborating with quants and traders across a \
        fully remote team distributed over European time zones.";

    fn linkedin_page() -> String {
        format!(
            "0 notifications\nSkip to search\nHome\nMy Network\nJobs\nLearning\n\n\
             Keyrock\n\nRust Engineer - Trading Systems\n\n\
             London, England, United Kingdom · 3 weeks ago · 13 people clicked apply\n\n\
             Responses managed off LinkedIn\n\nRemote\nFull-time\nApply\nSaved\n\
             Use AI to assess how you fit\n\nShow match details\n\nTailor my resume\n\n\
             People you can reach out to\n\nNicolas B\n\noptions trader\n\nMessage\n\
             About the job\n\nAbout Keyrock\n\n{FILLER}\n\n… more\n\n\
             Set alert for similar jobs\n\nSystem Engineer, London\n\n\
             Candidate seniority level\n\n62% Entry level candidates\n\n\
             More jobs\n\nSenior Rust Engineer | Remote\n\nLinkedIn Corporation © 2026\n\
             Select language\nDeutsch (German)\n"
        )
    }

    #[test]
    fn linkedin_keeps_header_and_description_drops_chrome() {
        let cleaned = clean("www.linkedin.com", &linkedin_page()).unwrap();
        // Header block survives: company, title, location line, work-mode chips.
        assert!(cleaned.starts_with("Keyrock\n\nRust Engineer - Trading Systems\n"));
        assert!(cleaned.contains("London, England, United Kingdom · 3 weeks ago"));
        assert!(cleaned.contains("Remote\nFull-time"));
        // Description survives, minus the trailing "… more" expander stub.
        assert!(cleaned.contains("About the job"));
        assert!(cleaned.contains("low-latency trading systems"));
        assert!(!cleaned.ends_with("… more"));
        assert!(cleaned.ends_with("European time zones."));
        // Nav, buttons, connection promos, tail modules, footer all gone.
        for noise in [
            "Skip to search",
            "My Network",
            "Use AI to assess",
            "Tailor my resume",
            "Nicolas B",
            "Set alert for similar jobs",
            "Entry level candidates",
            "LinkedIn Corporation",
            "Deutsch",
        ] {
            assert!(!cleaned.contains(noise), "expected {noise:?} to be stripped");
        }
    }

    #[test]
    fn linkedin_without_about_marker_is_left_alone() {
        assert_eq!(clean("www.linkedin.com", "some random page text"), None);
    }

    #[test]
    fn linkedin_tiny_result_is_left_alone() {
        let page = "Keyrock\n\nAbout the job\n\nShort.\n\nMore jobs\n";
        assert_eq!(clean("www.linkedin.com", page), None);
    }

    #[test]
    fn other_domains_are_untouched() {
        assert_eq!(clean("boards.example.com", &linkedin_page()), None);
    }

    #[test]
    fn cleaning_is_idempotent() {
        let once = clean("linkedin.com", &linkedin_page()).unwrap();
        match clean("linkedin.com", &once) {
            Some(twice) => assert_eq!(once, twice),
            None => {} // markers gone -> original kept, same outcome
        }
    }
}
