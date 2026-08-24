// Injected on demand when the user clicks Save, into every frame of the tab.
// Captures this frame's best candidate (see `extract.ts`) and hands it to the
// popup, which picks the winner across frames.
//
// The mode is read from storage rather than passed in: `executeScript` can
// only pass arguments to an inline function, and this script has to ship as a
// bundle because it carries Readability.

import { isCaptureMode, type CaptureMode } from "./capture";
import { capture } from "./extract";

async function loadMode(): Promise<CaptureMode> {
  try {
    const { mode } = await chrome.storage.local.get("mode");
    if (isCaptureMode(mode)) return mode;
  } catch {
    // Storage is unavailable in some sandboxed frames; auto is the default.
  }
  return "auto";
}

void (async () => {
  const payload = capture(await loadMode());
  // Rejects when the popup has already closed - nobody left to tell, which is
  // not an error worth surfacing in the page's console.
  void chrome.runtime.sendMessage({ type: "job-hunter:extracted", payload }).catch(() => {});
})();
