const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

function esc(v) {
  return String(v ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
}

function stat(label, value, mono) {
  return `<div class="stat"><div class="label">${esc(label)}</div><div class="value${mono ? " mono" : ""}">${esc(value)}</div></div>`;
}

function show(id) {
  for (const el of document.querySelectorAll("#onboardFolderView, #onboardSwarmView, #dashView")) {
    el.classList.toggle("d-none", el.id !== id);
  }
}

const TABS = ["details", "swarm", "media"];

function showTab(name) {
  for (const tab of TABS) {
    document.getElementById(`tabPanel-${tab}`).classList.toggle("d-none", tab !== name);
    document.getElementById(`tabBtn-${tab}`).classList.toggle("tab-active", tab === name);
  }
  if (name === "details") refreshDetails();
  if (name === "swarm") refreshSwarm();
  if (name === "media") refreshMedia();
}

for (const tab of TABS) {
  document.getElementById(`tabBtn-${tab}`).addEventListener("click", () => showTab(tab));
}

// ---- onboarding: media folder ---------------------------------------------

document.getElementById("chooseFolderBtn").addEventListener("click", async () => {
  const errorEl = document.getElementById("folderError");
  errorEl.textContent = "";
  try {
    const path = await invoke("choose_media_folder");
    if (path) {
      show("onboardSwarmView");
    }
  } catch (err) {
    errorEl.textContent = String(err);
  }
});

// ---- onboarding: optional swarm join ---------------------------------------

document.getElementById("onboardJoinBtn").addEventListener("click", async () => {
  const errorEl = document.getElementById("onboardError");
  errorEl.textContent = "";
  try {
    await invoke("join_swarm", {
      baseUrl: document.getElementById("onboardBaseUrl").value,
      code: document.getElementById("onboardCode").value,
      deviceName: document.getElementById("onboardDeviceName").value || "SWARM Server",
    });
    await enterDashboard();
  } catch (err) {
    errorEl.textContent = String(err);
  }
});

document.getElementById("onboardSkipBtn").addEventListener("click", enterDashboard);

// ---- boot -------------------------------------------------------------------

async function enterDashboard() {
  show("dashView");
  showTab("details");
}

async function boot() {
  const settings = await invoke("get_settings");
  if (!settings.media_roots || settings.media_roots.length === 0) {
    show("onboardFolderView");
    return;
  }
  await enterDashboard();
}

boot().catch(err => {
  document.getElementById("folderError").textContent = String(err);
  show("onboardFolderView");
});
