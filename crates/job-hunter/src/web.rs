//! HTML views over the same data `jobs`/`show` print to the terminal, served
//! by the same localhost server the browser extension posts jobs to.
//! `job-hunter jobs --web` / `show --web` open these instead of (or as well
//! as) printing to the terminal.
//!
//! Loopback-only and unauthenticated, same trust model as `/api/health`: a
//! plain browser navigation can't send the `X-Api-Token` header anyway, and
//! this server only ever binds `127.0.0.1`.

use crate::commands::jobs::{build_rows, JobsArgs, RowsResult};
use crate::commands::show::{self, fact_value_text, ShowData};
use crate::commands::{format_pct, format_salary, job_label};
use crate::server::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Html;
use job_hunter_core::assessment::schema::PreferenceMatch;
use job_hunter_core::config::Config;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// `text` split on blank lines into paragraphs, single newlines within a
/// paragraph kept as `<br>` - the HTML equivalent of `print_wrapped`.
fn paragraphs(text: &str) -> String {
    text.trim()
        .split("\n\n")
        .map(|p| format!("<p>{}</p>", esc(p.trim()).replace('\n', "<br>")))
        .collect()
}

const STYLE: &str = r#"
:root { color-scheme: light dark; }
body {
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  max-width: 960px;
  margin: 2rem auto;
  padding: 0 1rem;
  line-height: 1.5;
}
h1 { font-size: 1.4rem; font-weight: 600; }
h2 { font-size: 1.05rem; font-weight: 600; margin-top: 2rem; border-bottom: 1px solid color-mix(in srgb, currentColor 20%, transparent); padding-bottom: 0.3rem; }
table { border-collapse: collapse; width: 100%; margin: 1rem 0; font-size: 0.92rem; }
th, td { text-align: left; padding: 0.35rem 0.6rem; border-bottom: 1px solid color-mix(in srgb, currentColor 12%, transparent); white-space: nowrap; }
td.wrap { white-space: normal; }
th { font-weight: 600; color: color-mix(in srgb, currentColor 65%, transparent); }
tr:hover td { background: color-mix(in srgb, currentColor 5%, transparent); }
a { color: inherit; }
.meta { color: color-mix(in srgb, currentColor 60%, transparent); font-size: 0.9rem; }
.posting { white-space: pre-wrap; font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: 0.85rem; background: color-mix(in srgb, currentColor 4%, transparent); border-radius: 6px; padding: 1rem; max-height: 24rem; overflow-y: auto; }
.flag-red { color: #c0392b; }
.flag-green { color: #219150; }
ul { padding-left: 1.3rem; }
.summary { color: color-mix(in srgb, currentColor 60%, transparent); margin-top: 0.5rem; }
"#;

fn page(title: &str, body: &str) -> Html<String> {
    Html(format!(
        r#"<!doctype html><meta charset="utf-8"><title>{title}</title><style>{STYLE}</style>{body}"#,
        title = esc(title),
    ))
}

fn error_page(status_label: &str, message: &str) -> Html<String> {
    page(status_label, &format!("<h1>{}</h1><p>{}</p>", esc(status_label), esc(message)))
}

pub async fn jobs_page(
    State(state): State<AppState>,
    Query(args): Query<JobsArgs>,
) -> (StatusCode, Html<String>) {
    let db = state.db.clone();
    let config_file = state.config_file.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<RowsResult> {
        let config = Config::load(&config_file)?;
        let db = db.lock().expect("db mutex poisoned");
        build_rows(&db, &config, &args)
    })
    .await;
    match result {
        Ok(Ok(rows)) => (StatusCode::OK, page("Jobs", &render_jobs(&rows))),
        Ok(Err(e)) => {
            (StatusCode::BAD_REQUEST, error_page("400 Bad Request", &format!("{e:#}")))
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            error_page("500 Internal Server Error", "the request panicked"),
        ),
    }
}

pub async fn show_page(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> (StatusCode, Html<String>) {
    let db = state.db.clone();
    let config_file = state.config_file.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<ShowData> {
        let config = Config::load(&config_file)?;
        let db = db.lock().expect("db mutex poisoned");
        show::load(&db, &config, id)
    })
    .await;
    match result {
        Ok(Ok(data)) => {
            let title = job_label(&data.job);
            (StatusCode::OK, page(&title, &render_show(&data)))
        }
        Ok(Err(e)) => (StatusCode::NOT_FOUND, error_page("404 Not Found", &format!("{e:#}"))),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            error_page("500 Internal Server Error", "the request panicked"),
        ),
    }
}

fn render_jobs(result: &RowsResult) -> String {
    let mut out = String::from("<h1>Jobs</h1>");
    if result.rows.is_empty() {
        out.push_str("<p>No jobs to show.</p>");
        return out;
    }
    out.push_str("<table><thead><tr>");
    for h in [
        "ID", "Score", "CV", "Domain", "Pref", "✓", "Title", "Company", "Salary", "Mode",
        "Level", "Status", "Age",
    ] {
        out.push_str(&format!("<th>{h}</th>"));
    }
    out.push_str("</tr></thead><tbody>");
    for row in &result.rows {
        let check = match row.filters_passed {
            Some(true) => "✓",
            Some(false) => "✗",
            None => "·",
        };
        out.push_str(&format!(
            "<tr><td>{id}</td><td>{score}</td><td>{cv}</td><td>{domain}</td><td>{pref}</td><td>{check}</td>\
             <td class=\"wrap\"><a href=\"/jobs/{id}\">{title}</a></td><td>{company}</td><td>{salary}</td>\
             <td>{mode}</td><td>{level}</td><td>{status}</td><td>{age}</td></tr>",
            id = row.id,
            score = format_pct(row.score),
            cv = format_pct(row.cv_match),
            domain = format_pct(row.domain),
            pref = esc(&row.preference),
            title = esc(&row.title),
            company = esc(&row.company),
            salary = esc(&row.salary),
            mode = esc(&row.mode),
            level = esc(&row.level),
            status = esc(&row.status),
            age = esc(&row.age),
        ));
    }
    out.push_str("</tbody></table>");

    let unassessed = result.rows.iter().filter(|r| r.score.is_none()).count();
    let failing = result.rows.iter().filter(|r| r.filters_passed == Some(false)).count();
    let mut parts = vec![format!("{} shown", result.rows.len())];
    if result.hidden > 0 {
        parts.push(format!("{} filtered out", result.hidden));
    }
    if failing > 0 {
        parts.push(format!("{failing} shown failing the filters ✗"));
    }
    if result.trimmed > 0 {
        parts.push(format!("{} trimmed", result.trimmed));
    }
    if unassessed > 0 {
        parts.push(format!("{unassessed} not assessed"));
    }
    out.push_str(&format!("<p class=\"summary\">{}</p>", esc(&parts.join(" · "))));
    if result.hidden > 0 {
        out.push_str("<p class=\"meta\">Filtered-out jobs failed a hard filter under the current criteria.</p>");
    }
    out
}

fn render_show(data: &ShowData) -> String {
    let job = &data.job;
    let mut out = String::new();

    out.push_str(&format!("<h1>{} <span class=\"meta\">[{}]</span></h1>", esc(&job_label(job)), esc(&job.status)));
    out.push_str("<p class=\"meta\">");
    out.push_str(&format!("id {} &middot; <a href=\"{}\">{}</a><br>", job.id, esc(&job.url), esc(&job.url)));
    out.push_str(&format!(
        "domain {} &middot; source {} &middot; saved {}",
        esc(&job.domain),
        esc(&job.source),
        esc(&job.created_at)
    ));
    if let Some(location) = &job.location {
        out.push_str(&format!("<br>location {}", esc(location)));
    }
    out.push_str("</p>");
    if let Some(dup) = job.duplicate_of {
        out.push_str(&format!(
            "<p class=\"meta\">duplicate of <a href=\"/jobs/{dup}\">job #{dup}</a> (same title + company)</p>"
        ));
    }
    if !data.duplicates.is_empty() {
        out.push_str("<p class=\"meta\">also seen at:<ul>");
        for d in &data.duplicates {
            out.push_str(&format!(
                "<li><a href=\"{}\">{}</a> (<a href=\"/jobs/{}\">job #{}</a>)</li>",
                esc(&d.url),
                esc(&d.url),
                d.id,
                d.id
            ));
        }
        out.push_str("</ul></p>");
    }

    out.push_str("<h2>Original posting</h2>");
    out.push_str(&format!("<div class=\"posting\">{}</div>", esc(&job.content_text)));

    match &data.scored {
        None => {
            out.push_str(&format!(
                "<p>Not assessed yet - run <code>job-hunter assess {}</code>.</p>",
                job.id
            ));
        }
        Some((assessment, score)) => {
            out.push_str(&render_assessment(assessment, score));
        }
    }

    if !data.events.is_empty() {
        out.push_str("<h2>History</h2><table><tbody>");
        for e in &data.events {
            let note = e.note.as_deref().map(|n| format!(" - {n}")).unwrap_or_default();
            out.push_str(&format!(
                "<tr><td class=\"meta\">{}</td><td>{} &rarr; {}{}</td></tr>",
                esc(&e.created_at),
                esc(e.from_status.as_deref().unwrap_or("-")),
                esc(&e.to_status),
                esc(&note)
            ));
        }
        out.push_str("</tbody></table>");
    }

    if !data.feedback.is_empty() {
        out.push_str("<h2>Feedback</h2><table><tbody>");
        for f in &data.feedback {
            out.push_str(&format!(
                "<tr><td class=\"meta\">{}</td><td class=\"wrap\">{} <span class=\"meta\">[{}]</span></td></tr>",
                esc(&f.created_at),
                esc(&f.text),
                esc(&f.state)
            ));
        }
        out.push_str("</tbody></table>");
    }

    out
}

fn render_assessment(
    assessment: &job_hunter_core::db::Assessment,
    score: &job_hunter_core::db::Score,
) -> String {
    let mut out = String::new();
    let facts = match assessment.facts() {
        Ok(f) => f,
        Err(e) => return format!("<h2>Assessment</h2><p>could not read facts: {}</p>", esc(&format!("{e:#}"))),
    };
    let instr = assessment
        .instructions_hash
        .as_deref()
        .map(|h| format!(" / instr {h}"))
        .unwrap_or_default();
    out.push_str(&format!(
        "<h2>Assessment</h2><p class=\"meta\">experience {} / prompt v{}{} &middot; {}</p>",
        esc(&assessment.experience_hash[..8.min(assessment.experience_hash.len())]),
        assessment.prompt_version,
        esc(&instr),
        esc(&assessment.created_at)
    ));
    if let Some(summary) = &facts.extracted.summary {
        out.push_str(&paragraphs(summary));
    }
    out.push_str(&format!(
        "<p>salary: {} &nbsp; mode: {} &nbsp; type: {} &nbsp; seniority: {}</p>",
        esc(&format_salary(&facts)),
        esc(facts.extracted.work_mode.as_str()),
        esc(facts.extracted.employment_type.as_str()),
        esc(facts.extracted.seniority_level.as_str()),
    ));
    if !facts.extracted.tech_stack.is_empty() {
        out.push_str(&format!("<p>stack: {}</p>", esc(&facts.extracted.tech_stack.join(", "))));
    }
    if !facts.fit.skill_matches.is_empty() {
        let skills: Vec<String> = facts
            .fit
            .skill_matches
            .iter()
            .map(|m| format!("{} ({})", esc(&m.skill), esc(m.cv_evidence.as_str())))
            .collect();
        out.push_str(&format!("<p>skills vs CV: {}</p>", skills.join(", ")));
    }
    if !facts.fit.skills_missing.is_empty() {
        out.push_str(&format!("<p>missing: {}</p>", esc(&facts.fit.skills_missing.join(", "))));
    }
    for flag in &facts.fit.red_flags {
        out.push_str(&format!("<p class=\"flag-red\">&#9888; {}</p>", esc(flag)));
    }
    for flag in &facts.fit.green_flags {
        out.push_str(&format!("<p class=\"flag-green\">&#10022; {}</p>", esc(flag)));
    }
    if let Some(notes) = &facts.fit.notes {
        out.push_str(&format!("<p>notes: {}</p>", esc(notes)));
    }

    let suggestions = &facts.fit.suggestions;
    if !suggestions.is_empty() {
        out.push_str("<h2>How to approach this one</h2>");
        if let Some(advice) = &suggestions.advice {
            out.push_str(&paragraphs(advice));
        }
        for (heading, items) in [("Gaps", &suggestions.gaps), ("Strengths", &suggestions.strengths)] {
            if !items.is_empty() {
                out.push_str(&format!("<p><strong>{heading}</strong></p><ul>"));
                for item in items {
                    out.push_str(&format!("<li>{}</li>", esc(item)));
                }
                out.push_str("</ul>");
            }
        }
    }

    if facts.fit.preference_match != PreferenceMatch::Unspecified || facts.fit.preference_summary.is_some() {
        out.push_str(&format!(
            "<h2>Preference match: {}</h2>",
            esc(facts.fit.preference_match.as_str())
        ));
        if let Some(summary) = &facts.fit.preference_summary {
            out.push_str(&paragraphs(summary));
        }
        out.push_str("<p class=\"meta\">advisory - what you want does not move the score</p>");
    }

    let breakdown = match score.breakdown() {
        Ok(b) => b,
        Err(e) => return out + &format!("<p>could not read score breakdown: {}</p>", esc(&format!("{e:#}"))),
    };
    out.push_str(&format!(
        "<h2>Score: {}/100 &middot; hard filters: {}</h2>",
        format_pct(Some(score.score)),
        if score.hard_filter_passed { "passed &#10003;" } else { "FAILED &#10007;" }
    ));
    out.push_str("<p>the mean of the two fit measures, and nothing else:</p><ul>");
    out.push_str(&format!("<li>CV match &nbsp; {}</li>", format_pct(breakdown.cv_match)));
    out.push_str(&format!("<li>domain familiarity &nbsp; {}</li>", format_pct(breakdown.domain_familiarity)));
    out.push_str("</ul>");
    if !breakdown.filters.is_empty() {
        out.push_str(
            "<table><thead><tr><th>Hard filter</th><th>Requirement</th><th>Actual</th><th>Outcome</th></tr></thead><tbody>",
        );
        for f in &breakdown.filters {
            out.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&f.fact),
                esc(&format!("{:?} {}", f.op, f.value)),
                esc(&fact_value_text(&f.actual)),
                esc(&format!("{:?}", f.outcome)),
            ));
        }
        out.push_str("</tbody></table>");
    }
    out
}
