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

function stat(label, value, mono, infoId) {
  const clickable = infoId ? ` data-info="${infoId}" tabindex="0" role="button" class="stat stat-clickable"` : ` class="stat"`;
  const icon = infoId ? ` <i class="bi bi-info-circle info-affordance"></i>` : "";
  return `<div${clickable}><div class="label">${esc(label)}${icon}</div><div class="value${mono ? " mono" : ""}">${esc(value)}</div></div>`;
}

// ---- info modal ---------------------------------------------------------
// One shared "what am I looking at" popup for the whole app, opened by
// clicking (or Enter/Space-ing, for keyboard users) any element carrying
// data-info="<topicId>" — About tab's flow steps/feature tiles/badges,
// Details tab's stat tiles and card headers, AI tab's MCP heading and tool
// list. A single registry + single modal surface, same reasoning
// showToast() is one shared surface instead of bespoke status text per
// call site. Delegation (one listener on document), not a listener per
// element, since triggers live in both static markup (About, AI) and
// markup rebuilt on every refresh (Details' stat grid) — nothing needs to
// remember to re-wire anything after a re-render.
const INFO_TOPICS = {
  entries: {
    icon: "bi-collection-play", title: "Entries",
    body: "Every movie, episode, and track SWARM has found and catalogued across your media roots.",
  },
  "library-size": {
    icon: "bi-hdd-fill", title: "Library size",
    body: "The combined size on disk of every file in your library, across every media root.",
  },
  "listening-quic": {
    icon: "bi-broadcast", title: "Listening (QUIC)",
    body: "The address and port this server listens on for direct client connections. QUIC is a modern, encrypted transport protocol — the same one powering HTTP/3 — built to stay fast and reliable even on spotty networks.",
    link: "https://en.wikipedia.org/wiki/QUIC", linkLabel: "Learn about QUIC",
  },
  "upload-budget": {
    icon: "bi-speedometer2", title: "Streaming upload budget",
    body: "The share of your internet connection's upload speed this server allows itself to use for streaming, measured automatically so playback never saturates your home network.",
  },
  "active-sessions": {
    icon: "bi-play-circle-fill", title: "Active playback sessions",
    body: "How many clients are streaming from this server right now.",
  },
  "device-fingerprint": {
    icon: "bi-fingerprint", title: "Device fingerprint",
    body: "A unique hash of this server's security certificate, used so connecting devices can verify they're really talking to your server and not an impostor.",
    link: "https://en.wikipedia.org/wiki/Public_key_fingerprint", linkLabel: "Learn about certificate fingerprints",
  },
  "library-thumbprint": {
    icon: "bi-upc-scan", title: "Library thumbprint",
    body: "A single hash summarizing your whole library's contents, so clients can tell at a glance when anything's changed instead of re-checking every file.",
  },
  "media-roots": {
    icon: "bi-folder2-open", title: "Media roots",
    body: "The folders on this computer — or a mounted network share — that SWARM scans for movies, shows, and music. You can add more than one.",
  },
  "tmdb-scraping": {
    icon: "bi-cloud-download", title: "TMDb scraping",
    body: "TMDb (The Movie Database) is a free, community-built database SWARM uses to automatically fetch posters, artwork, cast lists, and plot summaries for your library.",
    link: "https://www.themoviedb.org/", linkLabel: "Visit TMDb",
  },
  "about-server": {
    icon: "bi-hdd-network-fill", title: "Your server",
    body: "Runs on your own computer, scans your media, and streams files directly to your devices — there's no cloud in between.",
  },
  "about-clients": {
    icon: "bi-tv-fill", title: "Your clients",
    body: "Fire TV today, with more platforms planned. A native app that connects straight to your server to browse and play your library.",
  },
  "about-secure": {
    icon: "bi-shield-lock-fill", title: "Secure by design",
    body: "Every device presents a certificate and proves who it is before it can connect — mutual verification over TLS 1.3, the same encryption standard used by online banking.",
    link: "https://en.wikipedia.org/wiki/Transport_Layer_Security", linkLabel: "Learn about TLS",
  },
  "about-no-cloud": {
    icon: "bi-cloud-slash-fill", title: "No cloud, ever",
    body: "Your files are never uploaded anywhere. Streaming happens directly between your own devices, so no third party ever stores or sees your media.",
  },
  "about-invite-only": {
    icon: "bi-key-fill", title: "Invite only",
    body: "New devices join with a short one-time code you generate yourself — there's no public sign-up, and you decide exactly who's allowed in.",
  },
  "about-merged-library": {
    icon: "bi-diagram-3-fill", title: "One library, everywhere",
    body: "Run more than one SWARM server? Every device in your swarm sees one combined library — the same file on two servers is merged automatically instead of showing up twice.",
  },
  "about-direct": {
    icon: "bi-wifi", title: "Direct device-to-device",
    body: "Every stream travels straight from your server to your client over a private connection — no third-party relay ever sits in the middle.",
    link: "https://en.wikipedia.org/wiki/Peer-to-peer", linkLabel: "Learn about peer-to-peer",
  },
  "mcp-protocol": {
    icon: "bi-stars", title: "What is MCP?",
    body: "The Model Context Protocol is an open standard that lets an AI assistant talk directly to outside tools and data. SWARM exposes a small, read-only MCP API so an assistant like Claude can look things up in your library on your behalf.",
    link: "https://modelcontextprotocol.io", linkLabel: "Read the MCP spec",
  },
  "tool-search-library": {
    icon: "bi-search", title: "search_library",
    body: "Finds entries in your library by title, kind, genre, rating, or liked status — the same filtering the Media tab's search box uses.",
  },
  "tool-get-entry-details": {
    icon: "bi-info-circle-fill", title: "get_entry_details",
    body: "Looks up everything known about one entry: its full synopsis, cast, rating, and genres.",
  },
  "tool-list-swarm-devices": {
    icon: "bi-diagram-3-fill", title: "list_swarm_devices",
    body: "Lists every device in your swarm and whether it's currently online — the same roster shown on the Swarm tab.",
  },
  "tool-list-client-errors": {
    icon: "bi-bell-fill", title: "list_client_errors",
    body: "Returns recent client-reported problems, like failed playback or an unreachable server — the same list shown on the Notifications tab.",
  },
};

function openInfoModal(topicId) {
  const topic = INFO_TOPICS[topicId];
  const backdrop = document.getElementById("infoModalBackdrop");
  if (!topic || !backdrop) return;
  document.getElementById("infoModalIcon").className = `bi ${topic.icon}`;
  document.getElementById("infoModalTitle").textContent = topic.title;
  document.getElementById("infoModalBody").textContent = topic.body;
  const link = document.getElementById("infoModalLink");
  if (topic.link) {
    link.href = topic.link;
    link.querySelector("span").textContent = topic.linkLabel || "Learn more";
    link.classList.remove("d-none");
  } else {
    link.classList.add("d-none");
  }
  backdrop.classList.remove("d-none");
  document.getElementById("infoModalClose").focus();
}

function closeInfoModal() {
  document.getElementById("infoModalBackdrop").classList.add("d-none");
}

document.getElementById("infoModalBackdrop").addEventListener("click", (e) => {
  if (e.target.id === "infoModalBackdrop") closeInfoModal();
});
document.getElementById("infoModalClose").addEventListener("click", closeInfoModal);

document.addEventListener("click", (e) => {
  const trigger = e.target.closest("[data-info]");
  if (trigger) openInfoModal(trigger.dataset.info);
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") { closeInfoModal(); return; }
  if (e.key === "Enter" || e.key === " ") {
    const trigger = e.target.closest && e.target.closest("[data-info]");
    if (trigger && trigger === e.target) {
      e.preventDefault();
      openInfoModal(trigger.dataset.info);
    }
  }
});

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

// "about" has no refresh*() dispatch below — its tab content is static
// (no invoke() calls, nothing that goes stale), unlike every other tab here.
const TABS = ["about", "details", "swarm", "notifications", "media", "ai"];

function showTab(name) {
  for (const tab of TABS) {
    document.getElementById(`tabPanel-${tab}`).classList.toggle("d-none", tab !== name);
    document.getElementById(`tabBtn-${tab}`).classList.toggle("tab-active", tab === name);
  }
  if (name === "details") refreshDetails();
  if (name === "swarm") refreshSwarm();
  if (name === "notifications") refreshNotifications();
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

// Notification bubble on the Notifications tab (client-reported error
// count) — kept here rather than in notifications.js despite belonging
// conceptually to that tab's feature, purely so it sits next to
// enterDashboard() below, the other thing that touches it at boot.
async function refreshNotificationBadge() {
  const badge = document.getElementById("notificationBadge");
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
  showTab("about");
  refreshNotificationBadge();
  setInterval(refreshNotificationBadge, 30000);
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
