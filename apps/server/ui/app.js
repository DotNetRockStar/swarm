const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

function esc(v) {
  return String(v ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
}

// ---- toast notifications ----------------------------------------------------
// Every success/warning/error message in the app surfaces as a toast rather
// than scattered inline text — see the doc comment on #toastStack in
// style.css. `type` picks the color/icon: "success" (green), "warning"
// (yellow), "error" (red, the default duration is longer since it's more
// likely worth reading in full before it disappears). Errors are never
// silently swallowed — every catch block in this app should route here.
const TOAST_ICONS = { success: "bi-check-circle-fill", warning: "bi-exclamation-triangle-fill", error: "bi-x-circle-fill" };

function showToast(message, type = "success", opts = {}) {
  const stack = document.getElementById("toastStack");
  if (!stack) return; // toast fired before the DOM's ready (shouldn't happen) — never throw over a notification
  const duration = opts.duration ?? (type === "error" ? 7000 : 4500);
  const toast = document.createElement("div");
  toast.className = `toast toast-${type}`;
  toast.innerHTML =
    `<i class="bi ${TOAST_ICONS[type] || TOAST_ICONS.success} toast-icon"></i>` +
    `<span class="toast-message"></span>` +
    `<button class="toast-close" aria-label="Dismiss"><i class="bi bi-x"></i></button>`;
  toast.querySelector(".toast-message").textContent = message;
  const remove = () => {
    toast.classList.add("toast-out");
    setTimeout(() => toast.remove(), 150);
  };
  toast.querySelector(".toast-close").addEventListener("click", remove);
  stack.appendChild(toast);
  if (duration > 0) setTimeout(remove, duration);
  return toast;
}

function stat(label, value, mono) {
  return `<div class="stat"><div class="label">${esc(label)}</div><div class="value${mono ? " mono" : ""}">${esc(value)}</div></div>`;
}

function show(id) {
  for (const el of document.querySelectorAll("#onboardFolderView, #onboardSwarmView, #dashView")) {
    el.classList.toggle("d-none", el.id !== id);
  }
  // Pairs with index.html's inline `body { visibility: hidden; }` guard —
  // see that comment. Idempotent and cheap, so just doing it unconditionally
  // on every show() (not only the first) keeps this single call site as the
  // one place that owns "is it safe to see the page yet" instead of a
  // separate first-call flag to track.
  document.body.style.visibility = "visible";
}

// "security" has no refresh*() dispatch below — its tab content is static
// (no invoke() calls, nothing that goes stale), unlike every other tab here.
const TABS = ["details", "swarm", "media", "ai", "security"];

function showTab(name) {
  for (const tab of TABS) {
    document.getElementById(`tabPanel-${tab}`).classList.toggle("d-none", tab !== name);
    document.getElementById(`tabBtn-${tab}`).classList.toggle("tab-active", tab === name);
  }
  if (name === "details") refreshDetails();
  if (name === "swarm") refreshSwarm();
  if (name === "media") refreshMedia();
  if (name === "ai") refreshAi();
}

for (const tab of TABS) {
  document.getElementById(`tabBtn-${tab}`).addEventListener("click", () => showTab(tab));
}

// ---- onboarding: media folder ---------------------------------------------

document.getElementById("chooseFolderBtn").addEventListener("click", async () => {
  try {
    const path = await invoke("choose_media_folder");
    if (path) {
      show("onboardSwarmView");
    }
  } catch (err) {
    showToast(String(err), "error");
  }
});

// ---- onboarding: optional swarm join ---------------------------------------

document.getElementById("onboardJoinBtn").addEventListener("click", async () => {
  try {
    await invoke("join_swarm", {
      baseUrl: document.getElementById("onboardBaseUrl").value,
      code: document.getElementById("onboardCode").value,
      deviceName: document.getElementById("onboardDeviceName").value || "SWARM Server",
    });
    await enterDashboard();
  } catch (err) {
    showToast(String(err), "error");
  }
});

document.getElementById("onboardSkipBtn").addEventListener("click", enterDashboard);

// ---- boot -------------------------------------------------------------------

// Notification bubble on the Swarm tab (client-reported error count) — kept
// here rather than in swarm.js despite belonging conceptually to that tab's
// feature, purely so it sits next to enterDashboard() below, the other
// thing that touches it at boot.
async function refreshErrorBadge() {
  const badge = document.getElementById("errorBadge");
  if (!badge) return;
  try {
    const count = Number(await invoke("client_error_count")) || 0;
    badge.textContent = count > 99 ? "99+" : String(count);
    badge.classList.toggle("d-none", count <= 0);
  } catch {
    // Best-effort background poll — a failed check isn't worth a toast every interval.
  }
}

async function enterDashboard() {
  show("dashView");
  showTab("details");
  refreshErrorBadge();
  setInterval(refreshErrorBadge, 30000);
}

async function boot() {
  const settings = await invoke("get_settings");
  if (!settings.media_roots || settings.media_roots.length === 0) {
    show("onboardFolderView");
    return;
  }
  await enterDashboard();
}

// Real bug, found live, twice: boot() (via enterDashboard() -> showTab())
// calls functions defined in details.js/swarm.js/media.js — every one of
// which loads *after* this file in index.html. Calling boot() unconditionally
// at this file's own top level, the way this used to work, is a genuine
// race, not a one-off fluke: each classic `<script>` tag gets a microtask
// checkpoint after it finishes running, and if invoke("get_settings")'s IPC
// round trip happens to resolve before the browser has fetched/parsed/run
// the remaining three script tags, boot()'s continuation calls a function
// that doesn't exist yet — first hit as `refreshErrorBadge` undefined, then
// again as `refreshDetails` undefined, both eventually caught by this same
// try/catch and misread as "settings didn't persist" (the catch's own
// fallback is to show onboarding) rather than what actually happened.
// DOMContentLoaded fixes the whole class at once, not just whichever
// function happened to race last: it only ever fires after every classic
// script in the document — all four files here — has finished executing,
// so nothing boot() reaches can possibly still be undefined by the time it
// runs. Regression test: apps/server/ui/test/boot_order.test.js.
document.addEventListener("DOMContentLoaded", () => {
  boot().catch(err => {
    showToast(String(err), "error");
    show("onboardFolderView");
  });
});
