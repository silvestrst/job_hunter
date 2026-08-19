# job-hunter

A personal job-search tool. Save postings from your browser with one click, have an LLM agent
assess each one against your CV, and get a ranked list driven by criteria *you* control —
where changing your criteria never costs another LLM call.

```
Chrome extension ──POST──> local server (Rust, 127.0.0.1) ──> SQLite
     (save click)                                               │
                                                                ▼
job-hunter assess ──> prompt (CV + feedback + job text) ──> LLM agent CLI
                                                       (claude / codex / any)
                                                                │
                                              criteria-independent facts JSON
                                                    stored in `assessments`
                                                                │
job-hunter jobs / rescore ──> pure-Rust scoring (YAML criteria) ──> ranking
```

## How it works, in depth

### The two-layer design

The central idea: **what the LLM knows about a job is separated from what you currently
want in a job.**

**Layer 1 — LLM-assessed fundamental data.** One agent call per job produces
*criteria-independent facts*: extracted fields (title, company, salary range with currency
and period, remote/hybrid/onsite, employment type, seniority, tech stack, …) plus
CV-relative fit measures (per-skill evidence from your CV, seniority fit,
`overall_cv_match`, red/green flags). The agent is explicitly forbidden from judging
against preferences — "salary is good" is banned; "£90k–£110k GBP/year" is a fact. This
JSON is preserved verbatim in the `assessments` table. It is the expensive artifact, and
it never goes stale when your preferences change.

**Layer 2 — pure-logic filtering and weighting.** Your YAML `criteria:` section defines
hard filters ("salary ≥ £85k", "remote or hybrid") and weighted soft criteria
("overall_cv_match weight 5, salary vs a £120k target weight 3, …"). A small Rust engine
evaluates them against the stored facts: sub-scores in 0..1 per criterion, weighted into a
0–100 score, with hard-filter failures still scored but hidden from the default view.
No LLM is involved anywhere in this layer.

### What triggers what

Every cached value is keyed by a hash of the thing it depends on:

| You change...            | Key that changes                | What happens                                     |
|--------------------------|---------------------------------|--------------------------------------------------|
| criteria in the YAML     | `criteria_hash` (16-hex)        | scores recompute instantly, **zero LLM calls**   |
| your CV file             | `cv_hash` (sha256 of the file)  | `assess` re-runs jobs against the new CV         |
| the prompt design        | `assessment.prompt_version`     | bump it in the config to force re-assessment     |
| your mind about a job    | nothing                         | record `feedback`; future assessments improve    |

`criteria_hash` is computed from the *parsed* criteria, so reformatting or commenting the
YAML doesn't invalidate anything — only semantic changes do. Scores are cached per
`(assessment, criteria_hash)`, so flipping back to previous criteria is also free.

### Job identity and dedupe

Two layers, because the same role appears in many places:

1. **URL layer (at save time).** URLs are normalized — lowercase host, no fragment,
   tracking params (`utm_*`, `gclid`, …) stripped, remaining query params sorted — and the
   normalized URL is unique. Re-saving the same page returns "already saved" instantly.
2. **Title+company layer (at assessment time).** Scraped page titles are too noisy for
   identity ("Senior Rust Engineer - Acme | LinkedIn"), so the authoritative title and
   company come from LLM extraction. When those backfill, a job matching an existing
   job's normalized title+company is linked via `duplicate_of` — the LinkedIn copy and
   the careers-page copy collapse into one job, with both URLs kept (`show` lists them).

### Status lifecycle

`new` → `assessed` (automatic after assessment), then user-driven: `applied` →
`progressing` / `rejected`. `dismissed` means *you* ruled it out (distinct from an
employer rejection). Every transition is recorded in `status_events` with an optional note.

## Setup

Prerequisites: Rust toolchain, Node.js (for building the extension once), and your
LLM agent CLI (default: [Claude Code](https://claude.com/claude-code)).

```sh
# 1. Build the binary
cargo build --release            # -> target/release/job-hunter

# 2. Create and edit the config
job-hunter config init           # writes ~/.config/job-hunter/config.yaml
$EDITOR ~/.config/job-hunter/config.yaml   # set resources.cv_path, tune criteria
job-hunter config check          # validates, prints resolved paths + criteria hash

# 3. Export your CV as markdown/plain text (once)
#    Google Docs / Word: File -> Download -> Markdown or Plain text.
#    Point resources.cv_path at it. PDFs are not supported by design.

# 4. Build and load the extension (once)
cd extension && npm install && npm run build
#    Chrome -> chrome://extensions -> Developer mode -> Load unpacked -> the extension/ dir

# 5. Run the server and connect the extension
job-hunter serve                 # prints the API token
#    Click the extension icon -> Settings -> paste the token -> Save settings

# 6. Optional: always-on server
job-hunter install-service --now # systemd user service; survives reboots
```

## Daily workflow

1. Browse job boards. On a posting you like, click the extension → **Save this job**.
2. When you have a batch: `job-hunter assess` (this is the only step that spends tokens).
3. `job-hunter jobs` — ranked table of what's worth applying to.
4. `job-hunter show 12` — why it scored what it scored: facts, filter results, per-criterion points.
5. Apply: `job-hunter status 12 applied --note "tailored CV v3"`.
6. Update as things move: `status 12 progressing`, `status 12 rejected`, `status 7 dismissed`.
7. Disagree with an assessment? `job-hunter feedback 12 "overall match is too high - I have no C++ and it's core here"`. Future assessments see it.
8. Preferences changed? Edit `criteria:` in the YAML, run `job-hunter rescore` — instant re-ranking, deltas shown, no tokens.

## Command reference

| Command | What it does |
|---|---|
| `jobs` | Ranked table (score desc). Default: statuses `new`+`assessed`, hard filters passed, duplicates hidden. |
| `jobs --all` / `--applied` / `--rejected` / `--progressing` / `--status <s>` | Other status views. |
| `jobs --include-filtered` | Also show hard-filter failures (marked ✗). |
| `jobs --min-score 70 --limit 10` | Trim the list. |
| `show <id>` | Full detail: extraction, fit, score breakdown tables, duplicate URLs, history, feedback. |
| `status <id> <status> [--note "..."]` | Transition with audit trail. |
| `assess [ids...] [--force]` | Run the LLM for jobs lacking a current assessment. `--force` re-runs (e.g. after feedback). |
| `rescore` | Recompute every score under current criteria. Instant; shows Δ vs previous criteria. |
| `feedback <id> "text"` | Job-linked feedback for the agent. |
| `feedback --general "text"` | Feedback not tied to one job. |
| `feedback --list` | Show distilled guidance + active items. |
| `feedback --compact` | Agent distills active feedback into ≤10 guidance rules (spends tokens, keeps prompts small). |
| `serve [--port N]` | Run the ingest server in the foreground (prints the API token). |
| `install-service [--now]` | Write the systemd user unit; `--now` also enables + starts it. |
| `config init [--force]` / `config check` | Write the commented default config / validate it. |
| `delete <id>` | Remove a mis-save (assessments and scores cascade). |

Planned (stage 2): `update` — a Google scraper that feeds the same ingest path with
`source: google`, so dedupe/assessment/scoring apply unchanged.

## Config reference

`~/.config/job-hunter/config.yaml` — every key, annotated:

```yaml
server:
  port: 7877                  # ingest server port; always binds 127.0.0.1 only

resources:
  cv_path: ~/cv.md            # your CV as markdown/plain text (~ expands)

agent:                        # LLM-agnostic invocation
  command: ["claude", "-p", "--output-format", "json", "--model", "{model}"]
                              # argv array, run directly (no shell).
                              # An element containing {prompt} gets substituted;
                              # otherwise the prompt is piped to stdin.
                              # An element containing {model} is substituted with
                              # `model` below.
  model: claude-sonnet-5      # pinned explicitly - required iff command references
                              # {model}. Without this, the CLI falls back to
                              # whatever model it's configured to use by default,
                              # which may not be the one you want to pay for.
  output: claude_json         # claude_json = unwrap Claude's JSON envelope (.result)
                              # text        = stdout is the answer itself
  timeout_secs: 180           # kill the agent after this long
  retries: 2                  # re-ask (with the validation error) on bad JSON

assessment:
  prompt_version: 1           # bump to force re-assessment of everything
  max_job_chars: 12000        # job text truncated beyond this (token lever)
  cv_verbatim_max_chars: 6000 # CV at/below this: embedded verbatim = lossless.
                              # Above: distilled ONCE by the agent (structure-
                              # preserving), cached, and the cache file at
                              # ~/.local/share/job-hunter/cv_digest/<hash>.txt
                              # is plain text you can review and hand-edit.
  cv_digest_max_words: 800    # word budget for that one-off distillation
  feedback_recent_n: 10       # recent feedback items included per prompt

criteria:                     # LAYER 2 - edits here never re-run the LLM
  currency_rates_to_gbp:      # normalize foreign salaries for comparison
    USD: 0.78
    EUR: 0.86
  hard_filters:               # all must pass for the default `jobs` view
    - {fact: salary_max_gbp, op: gte, value: 85000, if_missing: pass}
    - {fact: work_mode, op: in, value: [remote, hybrid], if_missing: fail}
    # ops: gte, lte, eq, in, not_in
    # if_missing: pass|fail - what to do when the posting didn't say
  soft:                       # weighted 0..1 sub-scores -> 0-100 total
    - {fact: overall_cv_match, weight: 5}            # already 0..1
    - {fact: seniority_fit, weight: 3}               # built-in: match=1.0 over=0.5 under=0.2
    - {fact: salary_max_gbp, weight: 3, target: 120000}  # clamp(value/target)
    - {fact: domain_familiarity, weight: 1}
    # - {fact: "skill:rust", weight: 4}              # built-in: strong=1.0 moderate=0.6 weak=0.3 none=0
    # - {fact: work_mode, weight: 2, map: {remote: 1.0, hybrid: 0.6}}  # enums need a map
    # - missing_score: 0.5   # optional per-criterion sub-score when the fact is missing
```

Valid `fact:` names: `salary_max_gbp`, `salary_min_gbp` (currency- and period-normalized
to GBP/year: day×220, hour×1840), `work_mode`, `employment_type`, `seniority_level`,
`seniority_fit`, `overall_cv_match`, `domain_familiarity`, `visa_sponsorship`, `country`,
`skill:<name>`. Typos are caught by `config check`, which lists these.

## The agent workspace

The agent CLI runs inside a prepared workspace directory (default
`~/.config/job-hunter/agent`, override with `agent.workspace_dir`) that holds its
instructions as standard conventions:

```
~/.config/job-hunter/agent/
  AGENTS.md                                  # source of truth: role, conventions, task index
  CLAUDE.md                                  # just "@AGENTS.md" (Claude Code import)
  .claude/skills/assess/SKILL.md             # per-task instructions (Agent Skills format)
  .claude/skills/cv-digest/SKILL.md
  .claude/skills/feedback-compact/SKILL.md
```

Each prompt carries only what changes per call — a task pointer, the JSON contract, the
CV, the feedback digest, and the job text. The instruction prose lives in these files:
edit them freely to tune behaviour, no rebuild needed. Files are created from built-in
defaults when missing (`config init` or first use) and never overwritten once present;
to reset one, delete it and rerun `config init`. `config check` shows which files you've
edited versus the shipped defaults.

The instructions in effect (AGENTS.md + the task's SKILL.md) are hashed into every
assessment, so editing them re-runs affected jobs on the next `assess` — exactly like a
`prompt_version` bump. Assessments made before this mechanism existed are grandfathered
and never silently re-run. One carve-out: an already-cached CV digest is keyed by CV
content only, so editing `cv-digest/SKILL.md` won't regenerate it — delete the cache
file under `~/.local/share/job-hunter/cv_digest/` to force that.

## Swapping the LLM agent

The agent contract: it receives one prompt (argv `{prompt}` placeholder, or stdin when
no placeholder is present), runs with the workspace above as its working directory, and
must print the answer to stdout — either wrapped in Claude's JSON envelope
(`output: claude_json`) or raw (`output: text`). JSON extraction is defensive (markdown
fences and surrounding prose are tolerated), and invalid output is retried with the
validation error appended.

```yaml
# Codex CLI instead of Claude Code:
agent:
  command: ["codex", "exec", "-m", "{model}", "--quiet", "{prompt}"]
  model: gpt-5-codex
  output: text
```

Anything works, including a shell script of your own — the test suite runs entirely
against `fixtures/mock-agent.sh`. Codex reads `AGENTS.md` natively; other CLIs reach the
same files through the prompt's pointer line. A backend that can't read files at all
still gets the JSON contract and data in every prompt (structure is enforced by the
validation-retry loop), it just misses the instruction prose. `model` and the `{model}`
placeholder are optional but tied together: `config check` (and `Config::validate`)
rejects one without the other, so there's no way to end up silently running whatever
model the CLI defaults to.

## Token economics

| Costs tokens | Free forever |
|---|---|
| `assess` (one call per job) | `jobs`, `show`, `rescore` |
| CV distillation (once, only if CV > `cv_verbatim_max_chars`) | any criteria edit + re-rank |
| `feedback --compact` (deliberate, manual) | recording feedback, status changes |
| editing agent workspace instructions (re-runs affected jobs on next `assess`) | |

Levers: `agent.model` is the biggest one - pick the cheapest model that still extracts
facts reliably; `max_job_chars` bounds the biggest prompt section; `cv_verbatim_max_chars`
/ `cv_digest_max_words` bound the CV section; `feedback_recent_n` bounds the feedback
section; the single-pass design does extraction *and* fit in one call; duplicates are
skipped by `assess`.

## The feedback loop

The agent is a public LLM — it can't be fine-tuned, so the improvement channel is the
prompt. Feedback is stored in SQLite and injected into every assessment prompt as: the
distilled guidance digest (if you've compacted), plus the `feedback_recent_n` most recent
items, each one line with job context so the lesson transfers. When active items pile up
(the CLI hints past 20), `feedback --compact` has the agent distill everything into ≤10
durable rules, keeping prompt overhead flat while retaining the lessons.

Feedback deliberately does **not** invalidate existing assessments (that would burn
tokens). It shapes future ones; `assess <id> --force` retro-applies it where you care.

## Troubleshooting

- **Extension says "server not running"** — start `job-hunter serve` (or install the
  service). The popup's dot is green when the server is reachable.
- **401 / token error in the popup** — paste the token printed by `job-hunter serve`
  (also shown by `config check`) into the extension's Settings.
- **`assess` fails with JSON validation errors** — the agent isn't returning the facts
  schema. Check `agent.output` matches your CLI (Claude Code: `claude_json`; most others:
  `text`). Claude Code's envelope format can drift between versions; re-check after
  upgrades.
- **Saved text is navigation junk or empty** — SPA job boards (LinkedIn, Workday) can
  defeat Readability. The popup warns when little text was extracted; per-site extractors
  are a known follow-up.
- **After `cargo install`/rebuild the service runs old code** —
  `systemctl --user restart job-hunter`.
- **Logs** — foreground: the terminal. Service: `journalctl --user -u job-hunter -f`.

## Development

```sh
cargo test                                   # entire Rust suite (no LLM needed)
cd extension && npm run typecheck && npm run build

# Manual server poke:
job-hunter serve &
TOKEN=$(cat ~/.local/share/job-hunter/api_token)
curl -s -X POST http://127.0.0.1:7877/api/jobs \
  -H "Content-Type: application/json" -H "X-Api-Token: $TOKEN" \
  --data @fixtures/payloads/sample1.json
```

`fixtures/` contains mock agents (`mock-agent.sh`, `-flaky`, `-fail`) that emit canned
Claude-style envelopes, sample payloads, canned facts, and a sample CV — the whole
pipeline is testable offline; the only step that ever needs a real LLM is a final smoke
test with your actual agent.

### Repository layout

```
crates/job-hunter-core/   library: config, db, ingest, urlnorm, assessment
                          (schema+prompt), agent runner, scoring, feedback
crates/job-hunter/        binary: clap CLI, axum server
extension/                Chrome MV3 extension (TypeScript, esbuild)
fixtures/                 mock agents, payloads, facts, sample CV
```

## Stage 2 roadmap

The Google scraper becomes an `update` command: search by configured queries, fetch and
text-extract results in Rust, and call the same `ingest_payload()` the server uses with
`source: google`. URL dedupe, title+company dedupe, assessment, and scoring all apply to
scraped jobs with no further changes.
