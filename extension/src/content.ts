// Injected on demand when the user clicks Save. Extracts the readable text of
// the current page with Readability (falling back to body text for pages it
// can't parse) and hands the payload back to the popup via a runtime message.

import { Readability } from "@mozilla/readability";

interface ExtractedPage {
  url: string;
  title: string;
  text: string;
}

function extract(): ExtractedPage {
  let text = "";
  let title = document.title;
  try {
    // Readability mutates its input, so parse a clone.
    const clone = document.cloneNode(true) as Document;
    const article = new Readability(clone).parse();
    if (article) {
      text = article.textContent ?? "";
      if (article.title) title = article.title;
    }
  } catch {
    // Fall through to body text.
  }
  const bodyText = document.body?.innerText ?? "";
  // SPA job boards often defeat Readability; take whichever is richer.
  if (bodyText.trim().length > text.trim().length) {
    text = bodyText;
  }
  return { url: location.href, title, text: text.trim() };
}

chrome.runtime.sendMessage({ type: "job-hunter:extracted", payload: extract() });
