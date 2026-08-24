// Tests for the page-capture ladder in src/extract.ts.
//
// The module is browser-only (it reaches for `document`, `location` and
// Readability), so it's bundled in memory with esbuild and run against a
// jsdom page - the same shape the content script sees in a real tab.

import assert from "node:assert/strict";
import { before, describe, test } from "node:test";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

let capture;
let htmlToText;

before(async () => {
  const { outputFiles } = await esbuild.build({
    entryPoints: ["src/extract.ts"],
    bundle: true,
    format: "esm",
    platform: "neutral",
    write: false,
  });
  const base64 = Buffer.from(outputFiles[0].contents).toString("base64");
  ({ capture, htmlToText } = await import(`data:text/javascript;base64,${base64}`));
});

/// Install a page as the globals `extract.ts` reads, optionally selecting the
/// contents of one element the way a user dragging over the posting would.
function mount(html, { url = "https://example.com/jobs/1", select } = {}) {
  const { window } = new JSDOM(html, { url });
  Object.assign(globalThis, {
    window,
    document: window.document,
    location: window.location,
    DOMParser: window.DOMParser,
    Element: window.Element,
  });
  if (select) {
    const range = window.document.createRange();
    range.selectNodeContents(window.document.querySelector(select));
    window.getSelection().removeAllRanges();
    window.getSelection().addRange(range);
  }
}

const DESCRIPTION =
  "<p>We are hiring a Senior Rust Engineer to own our low-latency trading stack " +
  "end to end.</p><ul><li>Design exchange connectivity</li>" +
  "<li>Operate market-data pipelines</li></ul><p>You will work with quants and " +
  "traders across a fully remote European team, shipping production systems " +
  "weekly and taking real ownership of the roadmap for the components you build.</p>";

const POSTING = {
  "@context": "https://schema.org",
  "@type": "JobPosting",
  title: "Senior Rust Engineer",
  description: DESCRIPTION,
  hiringOrganization: { "@type": "Organization", name: "Keyrock" },
  jobLocation: {
    "@type": "Place",
    address: {
      "@type": "PostalAddress",
      addressLocality: "London",
      addressRegion: "England",
      addressCountry: { "@type": "Country", name: "United Kingdom" },
    },
  },
  jobLocationType: "TELECOMMUTE",
  applicantLocationRequirements: [
    { "@type": "Country", name: "United Kingdom" },
    { "@type": "Country", name: "Germany" },
  ],
  employmentType: ["FULL_TIME"],
  baseSalary: {
    "@type": "MonetaryAmount",
    currency: "GBP",
    value: { "@type": "QuantitativeValue", minValue: 90000, maxValue: 120000, unitText: "YEAR" },
  },
  datePosted: "2026-08-01",
  validThrough: "2026-09-30",
  industry: "Financial Services",
  skills: ["Rust", "Linux", "Networking"],
};

/// A job page wrapped in the nav/promo/footer chrome that made whole-page
/// capture noisy in the first place.
function page(jsonLd) {
  const block = jsonLd
    ? `<script type="application/ld+json">${JSON.stringify(jsonLd)}</script>`
    : "";
  return `<!doctype html><html><head>
    <title>Senior Rust Engineer - Keyrock | LinkedIn | 40+ applicants</title>${block}
    </head><body>
    <nav>Home My Network Jobs Learning Notifications</nav>
    <aside>Set alert for similar jobs. More jobs. People you can reach out to.</aside>
    <main id="posting"><h1>Senior Rust Engineer</h1>
      <p>${"Body copy that Readability would find. ".repeat(20)}</p></main>
    <footer>LinkedIn Corporation 2026</footer></body></html>`;
}

const line = (text, label) => text.split("\n").find((l) => l.startsWith(`${label}:`));

describe("JobPosting structured data", () => {
  test("renders every field the facts contract cares about", () => {
    mount(page(POSTING));
    const result = capture("auto");
    assert.equal(result.source, "jsonld");
    assert.equal(line(result.text, "Job title"), "Job title: Senior Rust Engineer");
    assert.equal(line(result.text, "Company"), "Company: Keyrock");
    assert.equal(line(result.text, "Location"), "Location: London, England, United Kingdom");
    assert.equal(line(result.text, "Work mode"), "Work mode: Remote");
    assert.equal(line(result.text, "Remote restriction"), "Remote restriction: United Kingdom, Germany");
    assert.equal(line(result.text, "Employment type"), "Employment type: FULL_TIME");
    assert.equal(line(result.text, "Salary"), "Salary: 90000-120000 GBP per year");
    assert.equal(line(result.text, "Posted"), "Posted: 2026-08-01");
    assert.equal(line(result.text, "Closes"), "Closes: 2026-09-30");
    assert.equal(line(result.text, "Skills"), "Skills: Rust, Linux, Networking");
    assert.match(result.text, /- Design exchange connectivity/);
  });

  test("leaves the page chrome behind", () => {
    mount(page(POSTING));
    const { text } = capture("auto");
    for (const noise of ["My Network", "Set alert for similar jobs", "LinkedIn Corporation"]) {
      assert.doesNotMatch(text, new RegExp(noise), `expected ${noise} to be dropped`);
    }
  });

  test("titles the job from the data, not the page title", () => {
    mount(page(POSTING));
    assert.equal(capture("auto").title, "Senior Rust Engineer - Keyrock");
  });

  test("finds a posting inside a @graph, with @type as an array", () => {
    mount(page({
      "@context": "https://schema.org",
      "@graph": [
        { "@type": "WebSite", name: "Careers" },
        { ...POSTING, "@type": ["JobPosting"], hiringOrganization: "Acme Ltd" },
      ],
    }));
    const result = capture("auto");
    assert.equal(result.source, "jsonld");
    // schema.org lets any node be a bare string instead of an object.
    assert.equal(result.title, "Senior Rust Engineer - Acme Ltd");
  });

  test("finds a posting in a top-level array, and flattens repeated places", () => {
    mount(page([
      { "@type": "BreadcrumbList" },
      {
        ...POSTING,
        baseSalary: {
          currency: "EUR",
          value: { "@type": "QuantitativeValue", value: 85000, unitText: "YEAR" },
        },
        jobLocation: [
          { "@type": "Place", address: { addressLocality: "Berlin", addressCountry: "DE" } },
          { "@type": "Place", address: { addressLocality: "Berlin", addressCountry: "DE" } },
          { "@type": "Place", address: { addressLocality: "Paris", addressCountry: "FR" } },
        ],
      },
    ]));
    const { text } = capture("auto");
    assert.equal(line(text, "Salary"), "Salary: 85000 EUR per year");
    assert.equal(line(text, "Location"), "Location: Berlin, DE | Paris, FR");
  });

  test("a malformed block doesn't hide a valid one", () => {
    mount(`<!doctype html><html><head><title>t</title>
      <script type="application/ld+json">{ not json at all </script>
      <script type="application/ld+json">${JSON.stringify(POSTING)}</script>
      </head><body><main>x</main></body></html>`);
    assert.equal(capture("auto").source, "jsonld");
  });

  test("a teaser description keeps the fields but adds the page text", () => {
    mount(page({ ...POSTING, description: "<p>Great role. Apply now.</p>" }));
    const result = capture("auto");
    assert.equal(result.source, "jsonld+page");
    assert.match(result.text, /Company: Keyrock/);
    assert.match(result.text, /Body copy that Readability would find/);
  });
});

describe("capture ladder", () => {
  test("a selection beats the structured data", () => {
    mount(page(POSTING), { select: "#posting" });
    const result = capture("auto");
    assert.equal(result.source, "selection");
    assert.match(result.text, /Body copy that Readability would find/);
    assert.doesNotMatch(result.text, /My Network/);
    // The data is still parsed, because it titles the job far better than
    // "... | LinkedIn | 40+ applicants" does.
    assert.equal(result.title, "Senior Rust Engineer - Keyrock");
  });

  test("without structured data it falls back to the page", () => {
    mount(page(null));
    const result = capture("auto");
    assert.ok(["readable", "page"].includes(result.source), result.source);
    assert.match(result.text, /Body copy that Readability would find/);
    assert.match(result.title, /^Senior Rust Engineer - Keyrock \| LinkedIn/);
  });

  test("selection-only reports nothing rather than grabbing the page", () => {
    mount(page(POSTING));
    const result = capture("selection");
    assert.equal(result.source, "none");
    assert.equal(result.text, "");
  });

  test("selection-only returns the selection when there is one", () => {
    mount(page(POSTING), { select: "#posting" });
    const result = capture("selection");
    assert.equal(result.source, "selection");
    assert.match(result.text, /Body copy/);
  });

  test("whole-page mode ignores both the selection and the data", () => {
    mount(page(POSTING), { select: "#posting" });
    const result = capture("page");
    assert.equal(result.source, "page");
    assert.match(result.text, /My Network/);
  });
});

describe("htmlToText", () => {
  before(() => mount("<!doctype html><html><body></body></html>"));

  test("passes plain text through", () => {
    assert.equal(htmlToText("Just plain text."), "Just plain text.");
  });

  test("decodes entities", () => {
    assert.equal(htmlToText("R&amp;D &amp; ops"), "R&D & ops");
  });

  test("breaks on <br> and block elements", () => {
    assert.equal(htmlToText("a<br>b"), "a\nb");
    assert.equal(htmlToText("<p>one</p><p>two</p>"), "one\n\ntwo");
  });

  test("bullets list items on consecutive lines", () => {
    assert.equal(htmlToText("<ul><li>a</li><li>b</li></ul>"), "- a\n- b");
    assert.equal(htmlToText("<p>x</p><ul><li>a</li></ul><p>y</p>"), "x\n\n- a\n\ny");
  });

  test("drops scripts and styles", () => {
    assert.equal(htmlToText("<p>keep</p><script>drop()</script><style>i{}</style>"), "keep");
  });

  test("collapses runs of whitespace, including non-breaking spaces", () => {
    assert.equal(htmlToText("a   b"), "a b");
    assert.equal(htmlToText(""), "");
    assert.equal(htmlToText("   "), "");
  });
});
