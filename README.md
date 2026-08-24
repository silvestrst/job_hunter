# job-hunter

A personal job-search tool. Save postings from your browser with one click, have an LLM agent
assess each one against your experience, and get a list ranked by how well you fit — filtered
down to what you'd actually consider by rules *you* control, where changing those rules never
costs another LLM call.

```
Chrome extension ──POST──> local server (Rust, 127.0.0.1) ──> SQLite
     (save click)                                               │
                                                                ▼
job-hunter assess ──> prompt (profile + feedback + job text) ──> LLM agent CLI
                                                        (claude / codex / any)
                                                                │
                                              criteria-independent facts JSON
                                                    stored in `assessments`
                                                                │
job-hunter jobs / rescore ──> pure-Rust filtering (YAML criteria) ──> ranked, filtered list
```

## How it works, in depth

### The two-layer design

The central idea: **what the LLM knows about a job is separated from what you currently
want in a job.**

**Layer 1 — LLM-assessed fundamental data.** One agent call per job produces
*criteria-independent facts*: extracted fields (title, company, salary range with currency
and period, remote/hybrid/onsite, employment type, seniority, tech stack, …) plus
fit measures relative to your experience (per-skill evidence, seniority fit,
`overall_cv_match`, `domain_familiarity`, red/green flags). The agent is explicitly
forbidden from judging against preferences — "salary is good" is banned; "£90k–£110k
GBP/year" is a fact. This JSON is preserved verbatim in the `assessments` table. It is
the expensive artifact, and it never goes stale when your preferences change.

What you tell it about yourself is two documents. Your **experience** file is the
required one and the whole evidence base: roles, projects, scope, skills, context. It is
not a CV — nothing is summarised or truncated on the way into a prompt, so it is written
to be complete rather than to fit a page. The optional **preferences** file says what you
want from a role, and deliberately does *not* feed the fit measures. It gets its own
holistic verdict instead: the agent weighs the whole posting against everything you said
you wanted and grades it `poor` / `average` / `good` / `strong` / `exceptional`, with a
paragraph saying which wants it meets and which it misses. That grade is reported in
`jobs` and `show` and never enters the score — how well you fit a role and how much you
want it are different questions, and mixing them into one number hides both.

Both files are size-checked before they are opened — see `resources.max_file_bytes`
below. That guard is the only limit on the way in, and it exists so a mistyped path
cannot put something huge in a prompt, not to shave tokens.

**The score.** One question — *how well does this candidate fit this role* — answered by
the mean of the agent's two fit measures, `overall_cv_match` and `domain_familiarity`,
each shown beside it in `jobs` so the arithmetic is visible. Nothing else moves it. A
measure the agent couldn't produce is left out of the mean rather than counted as zero.
Salary, seniority, work mode, preference match and the rest are facts about the posting,
not measures of you, so they are reported and filtered on — never weighted in.

**Layer 2 — pure-logic filtering.** Your YAML `criteria:` section defines hard filters
("salary ≥ £85k", "remote or hybrid") over any stored fact. They are pass/fail, with no
grading anywhere: a filter decides whether a job is worth looking at, and the score
decides how it ranks among those that are. Failures keep their real score and are hidden
from the default `jobs` view, counted in its summary line, and shown with
`--include-filtered`. No LLM is involved anywhere in this layer.

### What triggers what

Every cached value is keyed by a hash of the thing it depends on:

| You change...            | Key that changes                | What happens                                     |
|--------------------------|---------------------------------|--------------------------------------------------|
| criteria in the YAML     | `criteria_hash` (16-hex)        | filters re-run instantly, **zero LLM calls**     |
| your experience file     | `experience_hash` (sha256)      | `assess` re-runs jobs against the new evidence   |
| your preferences file    | `preferences_hash` (16-hex)     | `assess` re-runs jobs against the new profile    |
| the prompt design        | `assessment.prompt_version`     | bump it in the config to force re-assessment     |
| your mind about a job    | nothing                         | record `feedback`; future assessments improve    |

`criteria_hash` is computed from the *parsed* criteria, so reformatting or commenting the
YAML doesn't invalidate anything — only semantic changes do. Scores and filter verdicts are
cached per `(assessment, criteria_hash)`, so flipping back to previous criteria is also free.
The score itself no longer depends on the criteria at all; what a criteria edit changes is
which jobs are shown.

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
$EDITOR ~/.config/job-hunter/config.yaml   # set resources.experience_path, tune criteria
job-hunter config check          # validates, prints resolved paths + criteria hash

# 3. Write your experience file as markdown/plain text (once)
#    Starting from a CV? Google Docs / Word: File -> Download -> Markdown or
#    Plain text, then add the detail the CV had no room for. PDFs are not
#    supported by design. Point resources.experience_path at it.

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
3. `job-hunter jobs` — ranked table of what's worth applying to, with the CV-match and
   domain columns the score is made of, your preference grade, and a summary line saying
   how many the filters took out.
4. `job-hunter show 12` — why it scored what it scored: facts, the two measures behind the
   score, the preference verdict in full, filter results.
5. Apply: `job-hunter status 12 applied --note "tailored CV v3"`. (`show` includes the
   assessment's own suggestions — how to approach that posting, plus the gaps and
   strengths the advice rests on.)
6. Update as things move: `status 12 progressing`, `status 12 rejected`, `status 7 dismissed`.
7. Disagree with an assessment? `job-hunter feedback 12 "overall match is too high - I have no C++ and it's core here"`. Future assessments see it.
8. Ruling something out wholesale? Edit `criteria.hard_filters` in the YAML, run
   `job-hunter rescore` — instant, shows which jobs the edit let in or pushed out, no tokens.
   (Changed your mind about what you *want*? That lives in your preferences file, and
   editing it re-runs `assess`.)

## Command reference

| Command | What it does |
|---|---|
| `jobs` | Ranked table (score desc), with the CV/domain columns behind the score and the preference grade. Default: statuses `new`+`assessed`, hard filters passed, duplicates hidden. Always ends with a `N shown · N filtered out · N not assessed` summary. |
| `jobs --all` / `--applied` / `--rejected` / `--progressing` / `--status <s>` | Other status views (these never hide filter failures — a job you've applied to shouldn't vanish because the criteria moved on). |
| `jobs --include-filtered` | Also show hard-filter failures (marked ✗). |
| `jobs --min-score 70 --limit 10` | Trim the list. |
| `show <id>` | Full detail: extraction, fit, preference verdict, the two measures behind the score, filter table, duplicate URLs, history, feedback. |
| `status <id> <status> [--note "..."]` | Transition with audit trail. |
| `assess [ids...] [--force]` | Run the LLM for jobs lacking a current assessment. `--force` re-runs (e.g. after feedback). |
| `rescore` | Re-run the hard filters for every assessed job under the current criteria. Instant; flags which verdicts flipped. |
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
  experience_path: ~/experience.md   # everything about you, as markdown/plain
                              # text (~ expands): roles, projects, scope, skills,
                              # context. Not a CV - it reaches the agent whole,
                              # so write the full detail. Required.
  preferences_path: ~/want.md # optional: what you want from a role, in your own
                              # words. Advisory: it earns its own holistic grade
                              # (poor..exceptional, shown in `jobs` and `show`)
                              # and shapes the agent's flags, notes and
                              # suggestions - never an extracted fact, a fit
                              # number, or the score. Absolute dealbreakers
                              # belong in `criteria.hard_filters` below. A
                              # configured-but-unreadable path is an error, not
                              # a silent skip.
  max_file_bytes: 8192        # the one size limit, and a safety guard rather than
                              # a token lever: any input file bigger than this is
                              # refused UNREAD, so a mistyped path can never put
                              # something huge in a prompt. Checked against size on
                              # disk, which for markdown is within a fraction of a
                              # percent of the character count. ~8000 characters.

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
  max_job_chars: 12000        # job text truncated beyond this (token lever).
                              # Postings arrive from the extension, not from a
                              # file you control, so this one truncates rather
                              # than refusing - see resources.max_file_bytes.
  feedback_recent_n: 10       # recent feedback items included per prompt

criteria:                     # LAYER 2 - filtering only; edits never re-run the LLM
  currency_rates_to_gbp:      # normalize foreign salaries for comparison
    USD: 0.78
    EUR: 0.86
  hard_filters:               # all must pass for the default `jobs` view
    - {fact: salary_max_gbp, op: gte, value: 85000, if_missing: pass}
    - {fact: work_mode, op: in, value: [remote, hybrid], if_missing: fail}
    # ops: gte, lte, eq, in, not_in
    # if_missing: pass|fail - what to do when the posting didn't say
    # - {fact: overall_cv_match, op: gte, value: 0.5}      # fit measures are 0..1
    # - {fact: preference_match, op: not_in, value: [poor], if_missing: pass}
    # - {fact: "skill:rust", op: in, value: [strong, moderate], if_missing: fail}
```

There are no weights, and no `soft:` section — the score is the mean of `overall_cv_match`
and `domain_familiarity` and nothing here can move it. A config still carrying `soft:` is
refused with an explanation rather than having its weights silently ignored.

Valid `fact:` names: `salary_max_gbp`, `salary_min_gbp` (currency- and period-normalized
to GBP/year: day×220, hour×1840), `work_mode`, `employment_type`, `seniority_level`,
`seniority_fit`, `overall_cv_match`, `domain_familiarity`, `preference_match`,
`visa_sponsorship`, `country`, `skill:<name>`. Typos are caught by `config check`, which
lists these.

## The agent workspace

The agent CLI runs inside a prepared workspace directory (default
`~/.config/job-hunter/agent`, override with `agent.workspace_dir`) that holds its
instructions as standard conventions:

```
~/.config/job-hunter/agent/
  AGENTS.md                                  # source of truth: role, conventions, task index
  CLAUDE.md                                  # just "@AGENTS.md" (Claude Code import)
  .claude/skills/assess/SKILL.md             # per-task instructions (Agent Skills format)
  .claude/skills/feedback-compact/SKILL.md
```

Each prompt carries only what changes per call — a task pointer, the JSON contract, the
experience, the feedback digest, and the job text. The instruction prose lives in these files:
edit them freely to tune behaviour, no rebuild needed. Files are created from built-in
defaults when missing (`config init` or first use); to reset one, delete it and rerun
`config init`.

**A file you have edited is never overwritten** — that is the whole contract of this
directory. A file you *haven't* edited belongs to the binary and is upgraded in place when
a new release changes it, because otherwise new instructions would never reach an existing
install. job-hunter tells them apart by hash, and anything it doesn't recognise counts as
yours. `config check` labels each file `default`, `outdated default` (will be refreshed),
or `edited`, and any refresh is announced on the run that performs it.

The instructions in effect — AGENTS.md, the task's SKILL.md, and the JSON contract the
prompt carries — are hashed into every assessment, so a change to any of them re-runs
affected jobs on the next `assess`, exactly like a `prompt_version` bump. (The contract is
in the binary rather than the workspace, and is hashed for precisely this reason: without
it, adding a field to the facts schema would leave every stored assessment looking current
while missing that field.) Assessments made before this mechanism existed are grandfathered
and never silently re-run.

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
| `feedback --compact` (deliberate, manual) | any criteria edit + re-rank |
| editing agent workspace instructions (re-runs affected jobs on next `assess`) | recording feedback, status changes |

Levers: `agent.model` is the biggest one - pick the cheapest model that still extracts
facts reliably; `max_job_chars` bounds the biggest prompt section; `feedback_recent_n`
bounds the feedback section; the single-pass design does extraction *and* fit in one
call; duplicates are skipped by `assess`. Your experience file is *not* a lever — it goes
in whole, every time, by design: it is the one input where losing detail costs accuracy
on every job. `resources.max_file_bytes` bounds it, but as a safety guard, not a budget.

## The feedback loop

The agent is a public LLM — it can't be fine-tuned, so the improvement channel is the
prompt. Feedback is stored in SQLite and injected into every assessment prompt as: the
distilled guidance digest (if you've compacted), plus the `feedback_recent_n` most recent
items, each one line with job context so the lesson transfers. When active items pile up
(the CLI hints past 20), `feedback --compact` has the agent distill everything into ≤10
durable rules, keeping prompt overhead flat while retaining the lessons.

Feedback deliberately does **not** invalidate existing assessments (that would burn
tokens). It shapes future ones; `assess <id> --force` retro-applies it where you care.

## The size guard

One number, `resources.max_file_bytes`, applies to every file you point the config at.
It is a safety guard, not a token budget: a file over it is refused **unread**, with an
error naming the path, both numbers and both remedies. The check is against the file's
size on disk, so nothing large is ever loaded to find out it was too large — and for
markdown, bytes track characters to within a fraction of a percent.

The failure it exists to catch is a mistyped or stale path — pointing `experience_path`
at a directory export, a database dump, or last year's 200-page notes file — where the
cost of finding out late is a broken prompt and a bill. `job-hunter config check` reports
each input file's size against the limit, so you can see the headroom before an `assess`
run rather than during one.

Nothing is truncated or summarised after the check passes. That is the trade: the guard
is strict precisely so everything inside it can travel whole.

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
Claude-style envelopes, sample payloads, canned facts, and a sample experience file — the whole
pipeline is testable offline; the only step that ever needs a real LLM is a final smoke
test with your actual agent.

### Repository layout

```
crates/job-hunter-core/   library: config, db, ingest, urlnorm, profile,
                          assessment (schema+prompt), agent runner, workspace,
                          scoring, feedback
crates/job-hunter/        binary: clap CLI, axum server
extension/                Chrome MV3 extension (TypeScript, esbuild)
fixtures/                 mock agents, payloads, facts, sample experience file
```

## Stage 2 roadmap

The Google scraper becomes an `update` command: search by configured queries, fetch and
text-extract results in Rust, and call the same `ingest_payload()` the server uses with
`source: google`. URL dedupe, title+company dedupe, assessment, and scoring all apply to
scraped jobs with no further changes.
