// Popup logic: health check on open, settings (token/port) in
// chrome.storage.local, and the save flow - inject the extractor into the
// active tab, receive the payload, POST it to the local server.

interface ExtractedPage {
  url: string;
  title: string;
  text: string;
}

interface Settings {
  token: string;
  port: number;
}

const DEFAULT_PORT = 7877;
const MIN_TEXT_CHARS = 200;

const dot = document.getElementById("dot") as HTMLDivElement;
const serverState = document.getElementById("server-state") as HTMLSpanElement;
const saveButton = document.getElementById("save") as HTMLButtonElement;
const message = document.getElementById("message") as HTMLDivElement;
const tokenInput = document.getElementById("token") as HTMLInputElement;
const portInput = document.getElementById("port") as HTMLInputElement;
const settingsSave = document.getElementById("settings-save") as HTMLButtonElement;
const settingsDetails = document.getElementById("settings") as HTMLDetailsElement;

function baseUrl(settings: Settings): string {
  return `http://127.0.0.1:${settings.port}`;
}

async function loadSettings(): Promise<Settings> {
  const stored = await chrome.storage.local.get(["token", "port"]);
  return {
    token: typeof stored.token === "string" ? stored.token : "",
    port: Number(stored.port) || DEFAULT_PORT,
  };
}

function show(kind: "ok" | "error" | "warn", text: string) {
  message.className = kind;
  message.textContent = text;
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

function extractFromActiveTab(): Promise<ExtractedPage> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      chrome.runtime.onMessage.removeListener(listener);
      reject(new Error("page extraction timed out"));
    }, 5000);
    const listener = (received: { type?: string; payload?: ExtractedPage }) => {
      if (received?.type !== "job-hunter:extracted" || !received.payload) return;
      clearTimeout(timer);
      chrome.runtime.onMessage.removeListener(listener);
      resolve(received.payload);
    };
    chrome.runtime.onMessage.addListener(listener);
    chrome.tabs
      .query({ active: true, currentWindow: true })
      .then(([tab]) => {
        if (!tab?.id) throw new Error("no active tab");
        if (!/^https?:/.test(tab.url ?? "")) {
          throw new Error("this page can't be saved (not an http/https page)");
        }
        return chrome.scripting.executeScript({
          target: { tabId: tab.id },
          files: ["dist/content.js"],
        });
      })
      .catch((e) => {
        clearTimeout(timer);
        chrome.runtime.onMessage.removeListener(listener);
        reject(e instanceof Error ? e : new Error(String(e)));
      });
  });
}

async function save() {
  const settings = await loadSettings();
  saveButton.disabled = true;
  show("ok", "Reading page...");
  try {
    const page = await extractFromActiveTab();
    if (page.text.length < MIN_TEXT_CHARS) {
      show("warn", "Couldn't extract much text - the posting may be behind a login. Saving what we got.");
    } else {
      show("ok", "Saving...");
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
    if (body.duplicate) {
      show("warn", `Already saved (job #${body.id}).`);
    } else {
      show("ok", `Saved as job #${body.id}. Assess it with:  job-hunter assess`);
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

async function init() {
  const settings = await loadSettings();
  tokenInput.value = settings.token;
  portInput.value = String(settings.port);
  if (!settings.token) {
    settingsDetails.open = true;
    show("warn", "Paste your API token below (printed by `job-hunter serve`).");
  }
  const up = await checkHealth(settings);
  saveButton.disabled = !up;
}

saveButton.addEventListener("click", () => void save());
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
