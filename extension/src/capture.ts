// Vocabulary shared by the popup and the injected extractor: what a capture
// is, where it came from, and which source wins. Deliberately free of DOM and
// Readability imports so the popup doesn't pull the parser into its bundle.

export type CaptureMode = "auto" | "selection" | "page";

export type CaptureSource =
  | "selection"
  | "jsonld"
  | "jsonld+page"
  | "readable"
  | "page"
  | "none";

export interface ExtractedPage {
  url: string;
  title: string;
  text: string;
  source: CaptureSource;
}

/// How much each source is trusted. The popup injects into every frame and
/// keeps the highest-ranked answer, so a posting embedded in an iframe (a
/// Greenhouse or Lever widget on a company careers page) beats the marketing
/// chrome wrapped around it.
export const SOURCE_RANK: Record<CaptureSource, number> = {
  selection: 100,
  jsonld: 80,
  "jsonld+page": 70,
  readable: 40,
  page: 20,
  none: 0,
};

export const SOURCE_LABEL: Record<CaptureSource, string> = {
  selection: "your selection",
  jsonld: "the posting's own job data",
  "jsonld+page": "job data + page text",
  readable: "the page's main article",
  page: "the whole page",
  none: "nothing",
};

export function isCaptureMode(value: unknown): value is CaptureMode {
  return value === "auto" || value === "selection" || value === "page";
}
