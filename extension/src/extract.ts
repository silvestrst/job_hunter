// Page capture: runs inside the tab, bundled into content.js.
//
// Capture is a ladder, best source first:
//
//   1. the user's selection - explicit intent always wins;
//   2. schema.org JobPosting structured data - site-agnostic and near
//      universal, because Google for Jobs indexing requires it;
//   3. Readability's article text;
//   4. raw body text, only when the steps above found nothing usable.
//
// Steps 1 and 2 exist because 3 and 4 drag in nav bars, promo rails and
// footers, which the model then has to read past.

import { Readability } from "@mozilla/readability";
import type { CaptureMode, CaptureSource, ExtractedPage } from "./capture";

/// Below this a Readability parse is treated as a failure and we fall back to
/// body text - the same threshold the popup uses to warn about thin captures.
const MIN_READABLE_CHARS = 200;

/// A JobPosting whose description is shorter than this is a teaser rather
/// than the posting; keep its structured fields but read the body off the
/// page as well. Set low on purpose - falling back drags the page chrome
/// into the capture, so only do it when the description is clearly a stub.
const MIN_DESCRIPTION_CHARS = 300;

export function capture(mode: CaptureMode): ExtractedPage {
  const url = location.href;
  if (mode === "page") {
    return { url, title: document.title, text: bodyText(), source: "page" };
  }

  // Parsed up front even for a selection: it carries a far better title than
  // `document.title`, which on most boards is "Role - Company | Site | 40+
  // applicants".
  const posting = findJobPosting();
  const title = postingTitle(posting) ?? document.title;

  const selection = (window.getSelection()?.toString() ?? "").trim();
  if (selection) {
    return { url, title, text: selection, source: "selection" };
  }
  if (mode === "selection") {
    return { url, title, text: "", source: "none" };
  }

  if (posting) {
    const header = renderPostingFields(posting);
    const description = htmlToText(asString(posting.description) ?? "");
    if (description.length >= MIN_DESCRIPTION_CHARS) {
      return { url, title, text: join(header, description), source: "jsonld" };
    }
    const [pageText] = readablePageText();
    return { url, title, text: join(header, description, pageText), source: "jsonld+page" };
  }

  const [text, source] = readablePageText();
  return { url, title, text, source };
}

/// Readability's article text, or raw body text when the parse came back
/// empty or implausibly short (single-page job boards routinely defeat it).
function readablePageText(): [string, CaptureSource] {
  let readable = "";
  try {
    // Readability mutates its input, so parse a clone.
    const article = new Readability(document.cloneNode(true) as Document).parse();
    readable = (article?.textContent ?? "").trim();
  } catch {
    // Fall through to body text.
  }
  if (readable.length >= MIN_READABLE_CHARS) return [readable, "readable"];
  const body = bodyText();
  return body.length > readable.length ? [body, "page"] : [readable, "readable"];
}

/// `innerText` is what we want - it respects layout, so hidden nodes stay out
/// and block elements keep their line breaks. It is missing in a few contexts
/// (XML documents, non-browser DOMs), where `textContent` is the only option
/// even though it keeps no line breaks.
function bodyText(): string {
  const body = document.body;
  if (!body) return "";
  const text = typeof body.innerText === "string" ? body.innerText : body.textContent;
  return (text ?? "").trim();
}

function join(...parts: string[]): string {
  return parts.filter((p) => p.trim()).join("\n\n").trim();
}

// --- schema.org JobPosting -------------------------------------------------

type Node = Record<string, unknown>;

/// The first JobPosting in any of the page's JSON-LD blocks. A block may be a
/// single node, an array, or a `@graph` wrapper, and `@type` may itself be an
/// array - all four shapes appear in the wild.
function findJobPosting(): Node | undefined {
  const nodes: Node[] = [];
  for (const script of Array.from(
    document.querySelectorAll('script[type="application/ld+json"]')
  )) {
    try {
      collect(JSON.parse(script.textContent ?? ""), nodes);
    } catch {
      // A malformed block is not a reason to give up on the others.
    }
  }
  return nodes.find(isJobPosting);
}

function collect(value: unknown, out: Node[]): void {
  if (Array.isArray(value)) {
    for (const item of value) collect(item, out);
    return;
  }
  if (!value || typeof value !== "object") return;
  const node = value as Node;
  out.push(node);
  if (node["@graph"]) collect(node["@graph"], out);
}

function isJobPosting(node: Node): boolean {
  const type = node["@type"];
  return Array.isArray(type) ? type.includes("JobPosting") : type === "JobPosting";
}

function postingTitle(posting: Node | undefined): string | undefined {
  if (!posting) return undefined;
  const title = asString(posting.title)?.trim();
  if (!title) return undefined;
  const company = name(posting.hiringOrganization);
  return company ? `${title} - ${company}` : title;
}

/// The structured fields as labelled lines. Left as text on purpose: the
/// facts contract is the model's job (layer 1), so this only has to be
/// faithful and legible, not normalised.
function renderPostingFields(posting: Node): string {
  const lines: string[] = [];
  const add = (label: string, value: string | undefined) => {
    if (value && value.trim()) lines.push(`${label}: ${value.trim()}`);
  };

  add("Job title", asString(posting.title));
  add("Company", name(posting.hiringOrganization));
  add("Location", locations(posting.jobLocation));
  if (asString(posting.jobLocationType) === "TELECOMMUTE") {
    add("Work mode", "Remote");
  }
  add("Remote restriction", names(posting.applicantLocationRequirements));
  add("Employment type", list(posting.employmentType));
  add("Salary", salary(posting.baseSalary));
  add("Posted", asString(posting.datePosted));
  add("Closes", asString(posting.validThrough));
  add("Industry", list(posting.industry));
  add("Category", list(posting.occupationalCategory));
  add("Experience required", htmlToText(asString(posting.experienceRequirements) ?? ""));
  add("Skills", list(posting.skills));

  return lines.join("\n");
}

/// MonetaryAmount: a currency plus either a QuantitativeValue range or a flat
/// value. Both forms are common; a bare number turns up too.
function salary(value: unknown): string | undefined {
  if (typeof value === "number") return String(value);
  if (!value || typeof value !== "object") return asString(value);
  const amount = value as Node;
  const inner = amount.value;
  let figure: string | undefined;
  let unit: string | undefined;
  if (typeof inner === "number" || typeof inner === "string") {
    figure = String(inner);
  } else if (inner && typeof inner === "object") {
    const q = inner as Node;
    const min = num(q.minValue);
    const max = num(q.maxValue);
    figure = min && max ? `${min}-${max}` : (min ?? max ?? num(q.value));
    unit = asString(q.unitText)?.toLowerCase();
  }
  if (!figure) return undefined;
  const currency = asString(amount.currency) ?? asString(amount.currencyCode);
  return [figure, currency, unit && `per ${unit}`].filter(Boolean).join(" ");
}

function num(value: unknown): string | undefined {
  if (typeof value === "number") return String(value);
  if (typeof value === "string" && value.trim()) return value.trim();
  return undefined;
}

/// A Place, or an array of them, flattened to "Locality, Region, Country".
function locations(value: unknown): string | undefined {
  const places = Array.isArray(value) ? value : [value];
  const rendered = places.map(place).filter((p): p is string => !!p);
  return rendered.length ? Array.from(new Set(rendered)).join(" | ") : undefined;
}

function place(value: unknown): string | undefined {
  if (!value) return undefined;
  if (typeof value === "string") return value;
  if (typeof value !== "object") return undefined;
  const address = (value as Node).address ?? value;
  if (typeof address === "string") return address;
  if (!address || typeof address !== "object") return undefined;
  const parts = ["addressLocality", "addressRegion", "addressCountry"]
    .map((key) => name((address as Node)[key]))
    .filter((p): p is string => !!p);
  return parts.length ? parts.join(", ") : name(value);
}

/// schema.org lets any node be a bare string or an object with a `name`.
function name(value: unknown): string | undefined {
  if (typeof value === "string") return value.trim() || undefined;
  if (!value || typeof value !== "object") return undefined;
  return asString((value as Node).name);
}

function names(value: unknown): string | undefined {
  const items = (Array.isArray(value) ? value : [value])
    .map(name)
    .filter((n): n is string => !!n);
  return items.length ? items.join(", ") : undefined;
}

function list(value: unknown): string | undefined {
  if (Array.isArray(value)) {
    const items = value.map(asString).filter((v): v is string => !!v);
    return items.length ? items.join(", ") : undefined;
  }
  return asString(value);
}

function asString(value: unknown): string | undefined {
  if (typeof value === "string") return value.trim() || undefined;
  if (typeof value === "number") return String(value);
  return undefined;
}

// --- HTML to text ----------------------------------------------------------

const BLOCK_TAGS = new Set([
  "ADDRESS", "ARTICLE", "ASIDE", "BLOCKQUOTE", "DD", "DIV", "DL", "DT",
  "FIELDSET", "FIGCAPTION", "FIGURE", "FOOTER", "FORM", "H1", "H2", "H3",
  "H4", "H5", "H6", "HEADER", "HR", "MAIN", "NAV", "OL", "P", "PRE",
  "SECTION", "TABLE", "TD", "TH", "TR", "UL",
]);

/// JobPosting descriptions are an HTML string. `innerText` would give the
/// nicest breaks but needs layout, and a parsed document is never rendered -
/// so walk it and put the breaks in by hand. Plain-text descriptions pass
/// through unharmed.
export function htmlToText(html: string): string {
  if (!html.trim()) return "";
  const doc = new DOMParser().parseFromString(html, "text/html");
  const out: string[] = [];
  const walk = (node: ChildNode): void => {
    if (node.nodeType === 3) {
      out.push(node.nodeValue ?? "");
      return;
    }
    if (node.nodeType !== 1) return;
    const el = node as Element;
    if (el.tagName === "SCRIPT" || el.tagName === "STYLE") return;
    if (el.tagName === "BR") {
      out.push("\n");
      return;
    }
    // A list item opens its own line but doesn't close one: the next item's
    // bullet or the enclosing list's break ends it, so bullets stay on
    // consecutive lines instead of being double-spaced.
    if (el.tagName === "LI") {
      out.push("\n- ");
      for (const child of Array.from(el.childNodes)) walk(child);
      return;
    }
    const block = BLOCK_TAGS.has(el.tagName);
    if (block) out.push("\n");
    for (const child of Array.from(el.childNodes)) walk(child);
    if (block) out.push("\n");
  };
  for (const child of Array.from(doc.body.childNodes)) walk(child);
  return out
    .join("")
    .replace(/[ \t\u00a0]+/g, " ")
    .replace(/ *\n[ \t]*/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}
