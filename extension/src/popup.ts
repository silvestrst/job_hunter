// Popup logic: health check on open, settings (token/port/capture mode) in
// chrome.storage.local, and the save flow - inject the extractor into every
// frame of the active tab, pick the best candidate, POST it to the local
// server.

import {
  isCaptureMode,
  SOURCE_LABEL,
  SOURCE_RANK,
  type CaptureMode,
  type ExtractedPage,
} from "./capture";

interface Settings {
  token: string;
  port: number;
  mode: CaptureMode;
}

const DEFAULT_PORT = 7877;
const MIN_TEXT_CHARS = 200;
/// How long to keep listening for stragglers after the first frame answers.
/// Frames that fail to inject never answer, so an exact count isn't enough.
const FRAME_GRACE_MS = 700;
const EXTRACT_TIMEOUT_MS = 5000;

const dot = document.getElementById("dot") as HTMLDivElement;
const serverState = document.getElementById("server-state") as HTMLSpanElement;
const saveButton = document.getElementById("save") as HTMLButtonElement;
const message = document.getElementById("message") as HTMLDivElement;
const modeSelect = document.getElementById("mode") as HTMLSelectElement;
const selectionState = document.getElementById("selection-state") as HTMLDivElement;
const tokenInput = document.getElementById("token") as HTMLInputElement;
const portInput = document.getElementById("port") as HTMLInputElement;
const settingsSave = document.getElementById("settings-save") as HTMLButtonElement;
const settingsDetails = document.getElementById("settings") as HTMLDetailsElement;
const framesToggle = document.getElementById("frames") as HTMLInputElement;

/// `activeTab` only grants host permission for the tab's main frame origin,
/// so a posting rendered by a widget from another site is invisible - both its
/// text and any selection inside it. Reaching into those frames needs broad
/// host access, which is worth asking for rather than assuming.
const ALL_URLS = { origins: ["<all_urls>"] };

function baseUrl(settings: Settings): string {
  return `http://127.0.0.1:${settings.port}`;
}

async function loadSettings(): Promise<Settings> {
  const stored = await chrome.storage.local.get(["token", "port", "mode"]);
  return {
    token: typeof stored.token === "string" ? stored.token : "",
    port: Number(stored.port) || DEFAULT_PORT,
    mode: isCaptureMode(stored.mode) ? stored.mode : "auto",
  };
}

function show(kind: "ok" | "error" | "warn", text: string) {
  message.className = kind;
  message.textContent = text;
}

function chars(n: number): string {
  return `${n.toLocaleString()} character${n === 1 ? "" : "s"}`;
}

async function checkHealth(settings: Settings): Promise<boolean> {
  try {
    const response = await fetch(`${baseUrl(settings)}/api/health`, {
      signal: AbortSignal.timeout(1500),
    });
    if (!response.ok) throw new Error(`status ${response.status}`);
    dot.className = "up";
    serverState.textContent = `server running on :${settings.port}`;
    return true;
  } catch {
    dot.className = "down";
    serverState.textContent = "server not running";
    show("error", "Start it with:  job-hunter serve");
    return false;
  }
}

async function activeTab(): Promise<chrome.tabs.Tab> {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (!tab?.id) throw new Error("no active tab");
  if (!/^https?:/.test(tab.url ?? "")) {
    throw new Error("this page can't be saved (not an http/https page)");
  }
  return tab;
}

/// Runs in the page, so it must be self-contained. Opening the popup does not
/// clear the page's selection, so this still sees what the user highlighted.
function probeSelection(): number {
  return (window.getSelection()?.toString() ?? "").trim().length;
}

/// Longest selection across the tab's frames. A posting shown through an
/// embedded widget lives in an iframe, where the top frame sees no selection
/// at all.
async function selectedChars(tabId: number): Promise<number> {
  const results = await injectAllFrames(tabId, { func: probeSelection });
  return results.reduce((max, r) => Math.max(max, (r.result as number) ?? 0), 0);
}

/// Inject into every frame, falling back to the top frame alone when the tab
/// refuses all-frame injection.
async function injectAllFrames(
  tabId: number,
  what: { func: () => number } | { files: string[] }
): Promise<chrome.scripting.InjectionResult<unknown>[]> {
  try {
    const results = await chrome.scripting.executeScript({
      target: { tabId, allFrames: true },
      ...what,
    } as chrome.scripting.ScriptInjection<unknown[], unknown>);
    if (results.length) return results;
  } catch {
    // Fall through to the top frame.
  }
  return chrome.scripting.executeScript({
    target: { tabId, frameIds: [0] },
    ...what,
  } as chrome.scripting.ScriptInjection<unknown[], unknown>);
}

/// Best candidate across frames: most trusted source first, then the longest
/// text among equally trusted ones.
function pickBest(candidates: ExtractedPage[]): ExtractedPage | undefined {
  return candidates
    .filter((c) => c.text.trim().length > 0 && c.source !== "none")
    .sort(
      (a, b) => SOURCE_RANK[b.source] - SOURCE_RANK[a.source] || b.text.length - a.text.length
    )[0];
}

function collectExtractions(tabId: number): Promise<ExtractedPage[]> {
  return new Promise((resolve, reject) => {
    const collected: ExtractedPage[] = [];
    let expected = Number.POSITIVE_INFINITY;
    let grace: ReturnType<typeof setTimeout> | undefined;
    let settled = false;

    const stop = () => {
      settled = true;
      clearTimeout(hard);
      if (grace) clearTimeout(grace);
      chrome.runtime.onMessage.removeListener(listener);
    };
    const finish = () => {
      if (settled) return;
      stop();
      resolve(collected);
    };
    const fail = (error: Error) => {
      if (settled) return;
      stop();
      reject(error);
    };

    const hard = setTimeout(() => {
      if (collected.length) finish();
      else fail(new Error("page extraction timed out"));
    }, EXTRACT_TIMEOUT_MS);

    const listener = (received: { type?: string; payload?: ExtractedPage }) => {
      if (received?.type !== "job-hunter:extracted" || !received.payload) return;
      collected.push(received.payload);
      if (collected.length >= expected) return finish();
      if (!grace) grace = setTimeout(finish, FRAME_GRACE_MS);
    };
    chrome.runtime.onMessage.addListener(listener);

    injectAllFrames(tabId, { files: ["dist/content.js"] })
      .then((frames) => {
        expected = frames.length;
        if (collected.length >= expected) finish();
      })
      .catch((e) => {
        // Frames that already answered are still worth saving.
        if (collected.length) finish();
        else fail(e instanceof Error ? e : new Error(String(e)));
      });
  });
}

async function save() {
  const settings = await loadSettings();
  saveButton.disabled = true;
  show("ok", "Reading page...");
  try {
    const tab = await activeTab();
    const page = pickBest(await collectExtractions(tab.id!));
    if (!page) {
      show(
        "error",
        settings.mode === "selection"
          ? "Nothing is selected. Highlight the posting on the page, then click Save."
          : "Couldn't read any text from this page."
      );
      return;
    }
    if (page.text.length < MIN_TEXT_CHARS) {
      show(
        "warn",
        page.source === "selection"
          ? `Only ${chars(page.text.length)} selected - saving anyway.`
          : "Couldn't extract much text - the posting may be behind a login. Saving what we got."
      );
    } else {
      show("ok", `Saving ${chars(page.text.length)} from ${SOURCE_LABEL[page.source]}...`);
    }
    const response = await fetch(`${baseUrl(settings)}/api/jobs`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Api-Token": settings.token,
      },
      body: JSON.stringify({
        url: page.url,
        title: page.title,
        text: page.text,
        source: "extension",
      }),
    });
    if (response.status === 401) {
      show("error", "Token missing or wrong - paste it in Settings below (shown by `job-hunter serve`).");
      settingsDetails.open = true;
      return;
    }
    const body = (await response.json()) as { id?: number; duplicate?: boolean; error?: string };
    if (!response.ok) {
      show("error", `Server rejected the save: ${body.error ?? response.status}`);
      return;
    }
    const captured = `${SOURCE_LABEL[page.source]}, ${chars(page.text.length)}`;
    if (body.duplicate) {
      show("warn", `Already saved (job #${body.id}).\nCaptured ${captured}.`);
    } else {
      show(
        "ok",
        `Saved as job #${body.id} (${captured}).\nAssess it with:  job-hunter assess`
      );
    }
  } catch (e) {
    const reason = e instanceof Error ? e.message : String(e);
    if (reason.includes("Failed to fetch")) {
      show("error", "Server not running - start it with:  job-hunter serve");
    } else {
      show("error", reason);
    }
  } finally {
    saveButton.disabled = false;
  }
}

/// Tell the user what will actually be captured before they commit to it -
/// otherwise a selection that was lost (a stray click clears it) silently
/// turns into a whole-page grab.
async function refreshSelectionState(mode: CaptureMode) {
  if (mode === "page") {
    selectionState.className = "";
    selectionState.textContent = "Will capture the whole page.";
    return;
  }
  selectionState.className = "";
  selectionState.textContent = "checking selection...";
  try {
    const tab = await activeTab();
    const selected = await selectedChars(tab.id!);
    if (selected > 0) {
      selectionState.className = "ok";
      selectionState.textContent = `${chars(selected)} selected.`;
    } else if (mode === "selection") {
      selectionState.className = "warn";
      selectionState.textContent = "Nothing selected - highlight the posting first.";
    } else {
      selectionState.textContent = "Nothing selected - will look for the posting itself.";
    }
  } catch {
    selectionState.textContent = "";
  }
}

async function init() {
  const settings = await loadSettings();
  tokenInput.value = settings.token;
  portInput.value = String(settings.port);
  modeSelect.value = settings.mode;
  framesToggle.checked = await chrome.permissions.contains(ALL_URLS);
  if (!settings.token) {
    settingsDetails.open = true;
    show("warn", "Paste your API token below (printed by `job-hunter serve`).");
  }
  const up = await checkHealth(settings);
  saveButton.disabled = !up;
  await refreshSelectionState(settings.mode);
}

saveButton.addEventListener("click", () => void save());
framesToggle.addEventListener("change", () => {
  void (async () => {
    // Must run inside the click that flipped the box - Chrome only shows the
    // permission prompt for a user gesture.
    const granted = framesToggle.checked
      ? await chrome.permissions.request(ALL_URLS)
      : !(await chrome.permissions.remove(ALL_URLS));
    framesToggle.checked = granted;
    await refreshSelectionState(modeSelect.value as CaptureMode);
  })();
});
modeSelect.addEventListener("change", () => {
  void (async () => {
    const mode = modeSelect.value as CaptureMode;
    await chrome.storage.local.set({ mode });
    await refreshSelectionState(mode);
  })();
});
settingsSave.addEventListener("click", () => {
  void (async () => {
    await chrome.storage.local.set({
      token: tokenInput.value.trim(),
      port: Number(portInput.value.trim()) || DEFAULT_PORT,
    });
    show("ok", "Settings saved.");
    await init();
  })();
});

void init();
