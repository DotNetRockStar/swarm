#!/usr/bin/env node
//
// Regression test for a real bug found live *twice*: app.js's boot()
// (via enterDashboard() -> showTab("media")) calls functions defined in
// details.js/swarm.js/media.js -- every one of which loads *after* app.js
// in index.html's own <script> order. Calling boot() unconditionally at
// app.js's own top level (how this used to work) races the browser's script
// loading: each classic <script> tag gets a microtask checkpoint right
// after it finishes running, and if the get_settings() IPC round trip
// happens to resolve before the browser has fetched/parsed/run the
// remaining three <script> tags, boot()'s continuation calls a function
// that doesn't exist yet. First hit as `refreshErrorBadge` undefined, then
// again as `refreshDetails` undefined -- both landed in boot()'s own
// try/catch and got misread as "settings didn't persist" (the catch's
// fallback shows onboarding) rather than what actually happened.
//
// The fix (app.js wraps its boot() call in a DOMContentLoaded listener,
// which only ever fires after every classic script in the document has
// finished executing) makes this ordering impossible by construction. This
// test loads the *real* index.html and the *real* four script files in a
// jsdom window with an invoke() stub that resolves as fast as a Promise can
// possibly resolve -- the most adversarial timing available -- and fails
// loudly if that race is ever reintroduced (e.g. someone "simplifies" the
// DOMContentLoaded wrapper back out, or a new script tag gets added in the
// wrong order).
//
// Run: cd apps/server/ui/test && npm install && npm test

const { JSDOM } = require("jsdom");
const fs = require("fs");
const path = require("path");

const UI_DIR = path.join(__dirname, "..");
const invokeCalls = [];
let testLibraryEntries = [];
let testRootHealth = [{ label: "test", path: "/tmp/swarm-boot-order-test", available: true, error: null }];
let testServerNotifications = [];
let copiedText = null;

function invokeStub(command, args) {
  invokeCalls.push({ command, args });
  switch (command) {
    case "get_settings":
      // Non-empty media_roots is what sends boot() down the enterDashboard()
      // path -- an empty/missing one just shows onboarding, never reaching
      // any of the functions this test exists to guard.
      return {
        media_roots: [{ label: "test", path: "/tmp/swarm-boot-order-test" }],
        has_tmdb_key: false,
        streaming_upload_budget_enabled: true,
        local_transcription_enabled: false,
        transcription_pause_while_streaming: true,
        mcp_enabled: false,
        mcp_port: 7890,
        mcp_access_token: null,
      };
    case "get_status":
      return {
        fingerprint: "test-fingerprint",
        media_roots: ["test: /tmp/swarm-boot-order-test"],
        listen_addr: "0.0.0.0:8543",
        entry_count: 0,
        thumbprint: "0".repeat(32),
        streaming_upload_budget_bps: 1000000,
        streaming_upload_budget_enabled: true,
        active_playback_sessions: 0,
        scanning: false,
      };
    case "get_media_root_health":
      return testRootHealth;
    case "get_transcription_status":
      return {
        enabled: false,
        phase: "disabled",
        message: "Local subtitle generation is disabled.",
        model_name: "small.en",
        model_installed: false,
        downloaded_bytes: 0,
        download_total_bytes: 0,
        queued: 0,
        completed: 0,
        failed: 0,
        total_segments: 0,
        completed_segments: 0,
        current_title: null,
        current_segment: 0,
        current_total_segments: 0,
        current_segment_progress: 0,
      };
    case "list_entries":
      return testLibraryEntries;
    case "list_media_roots":
    case "list_categories":
    case "list_client_errors":
      return [];
    case "list_server_notifications":
      return testServerNotifications;
    case "client_error_count":
    case "notification_count":
      return 0;
    case "get_swarm_link":
      return null;
    default:
      return {};
  }
}

/** Minimal fake -- just enough that media.js's module-scope `new IntersectionObserver(...)` doesn't throw; this test never scrolls anything into view. */
class FakeIntersectionObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

async function main() {
  const html = fs.readFileSync(path.join(UI_DIR, "index.html"), "utf8");
  const consoleErrors = [];
  const windowErrors = [];

  const dom = new JSDOM(html, {
    // file:// so index.html's relative <script src="app.js"> etc. resolve
    // to real files on disk via jsdom's own resource loader, exactly like a
    // browser would resolve them relative to the page's own URL.
    url: `file://${path.join(UI_DIR, "index.html")}`,
    runScripts: "dangerously",
    resources: "usable",
    pretendToBeVisual: true,
    beforeParse(window) {
      // Must exist before app.js's own top-level `window.__TAURI__.core.invoke`
      // reference runs -- beforeParse fires before any parsing/script
      // execution starts, so this is the only correct place to install it.
      window.__TAURI__ = {
        core: { invoke: (command, args) => Promise.resolve(invokeStub(command, args)) },
        event: { listen: () => Promise.resolve(() => {}) },
      };
      window.IntersectionObserver = FakeIntersectionObserver;
      window.navigator.clipboard = {
        writeText: (text) => {
          copiedText = text;
          return Promise.resolve();
        },
      };
      window.addEventListener("error", (event) => {
        const err = event.error;
        windowErrors.push(err && err.stack ? err.stack : String(event.message || err));
      });
    },
  });

  dom.window.console.error = (...args) => consoleErrors.push(args.map(String).join(" "));

  // DOMContentLoaded (and hence boot()) fires synchronously as part of
  // jsdom parsing/running the document's scripts when runScripts:
  // "dangerously" is set -- by the time the JSDOM constructor above
  // returns, every classic <script> tag (including this one) has already
  // executed. What's still pending is boot()'s own async continuation
  // (the get_settings() promise chain) and enterDashboard()'s further
  // async work, so a short drain of the microtask/timer queue is what's
  // actually being waited on here, not script loading.
  await new Promise((resolve) => setTimeout(resolve, 200));

  const { document } = dom.window;
  const dashVisible = !document.getElementById("dashView").classList.contains("d-none");
  const onboardVisible = !document.getElementById("onboardFolderView").classList.contains("d-none");
  const toastTexts = [...document.querySelectorAll(".toast-message")].map((el) => el.textContent);
  const referenceErrors = [...windowErrors, ...consoleErrors, ...toastTexts].filter((msg) => /ReferenceError/i.test(msg));

  const failures = [];
  if (referenceErrors.length > 0) {
    failures.push(`ReferenceError(s) during boot: ${referenceErrors.join(" | ")}`);
  }
  if (windowErrors.length > 0) {
    failures.push(`Uncaught error(s) during boot: ${windowErrors.join(" | ")}`);
  }
  if (!dashVisible) {
    failures.push(`Expected #dashView to be visible after boot (a real, persisted media_roots settings response) but it was hidden${onboardVisible ? " -- fell back to onboarding, the exact symptom of this bug class" : ""}.`);
  }
  const expectedTabOrder = ["tabBtn-media", "tabBtn-details", "tabBtn-swarm", "tabBtn-notifications", "tabBtn-ai", "tabBtn-about"];
  const actualTabOrder = [...document.querySelectorAll(".tabnav > button[id^='tabBtn-']")]
    .map((button) => button.id);
  if (JSON.stringify(actualTabOrder) !== JSON.stringify(expectedTabOrder)) {
    failures.push(`Expected tab order ${expectedTabOrder.join(", ")}, got ${actualTabOrder.join(", ")}.`);
  }
  if (!document.getElementById("tabBtn-media").classList.contains("tab-active")) {
    failures.push("Expected Media to be the active default tab after boot.");
  }
  if (document.getElementById("tabPanel-media").classList.contains("d-none")) {
    failures.push("Expected the Media panel to be visible after boot.");
  }
  if (!document.getElementById("tabPanel-about").classList.contains("d-none")) {
    failures.push("Expected the About panel to be hidden when Media is the default.");
  }
  if (document.getElementById("transcriptionProgress").classList.contains("d-none")) {
    failures.push("Expected local subtitle progress to remain visible on the Media tab while disabled.");
  }
  if (!document.getElementById("transcriptionProgressText").textContent.includes("disabled")) {
    failures.push("Expected the Media subtitle panel to render the current durable-worker status.");
  }
  // Companion regression: index.html's inline `body { visibility: hidden }`
  // guard (a flash-of-onboarding fix, same "wrong view painted first" family
  // as this file's main bug) only ever gets lifted by show() -- if body
  // stayed hidden, the user would see nothing at all rather than the wrong
  // view, but it's the same "did boot() actually finish deciding what to
  // show" question this whole file is about.
  if (dom.window.getComputedStyle(document.body).visibility !== "visible") {
    failures.push("Expected document.body to be revealed (visibility: visible) after boot, but it was still hidden.");
  }
  if (document.getElementById("statusGrid").textContent.includes("Listening (QUIC)")) {
    failures.push("Expected the Listening (QUIC) status panel to be removed.");
  }
  if (!document.getElementById("mediaRootWarning").classList.contains("d-none")) {
    failures.push("Expected the media-root warning to stay hidden while every root is readable.");
  }
  testRootHealth = [{ label: "nas", path: "/Volumes/missing-share/movies", available: false, error: "not found" }];
  await dom.window.refreshMediaRootHealth();
  if (document.getElementById("mediaRootWarning").classList.contains("d-none")) {
    failures.push("Expected an unavailable media root to show the persistent storage warning.");
  }
  if (!document.getElementById("mediaRootWarningText").textContent.includes("/Volumes/missing-share/movies")) {
    failures.push("Expected the storage warning to identify the unavailable media-root path.");
  }
  testRootHealth = [{ label: "nas", path: "/Volumes/missing-share/movies", available: true, error: null }];
  await dom.window.refreshMediaRootHealth();
  if (!document.getElementById("mediaRootWarning").classList.contains("d-none")) {
    failures.push("Expected the storage warning to clear automatically after the root recovers.");
  }
  document.querySelector('[data-info="try-asking"]').click();
  if (document.getElementById("infoModalBackdrop").classList.contains("d-none")) {
    failures.push("Expected clicking Try asking to open its information modal.");
  }
  if (document.querySelectorAll("#infoModalLinks a").length !== 2) {
    failures.push("Expected Try asking help to offer both Codex and Claude links.");
  }

  testServerNotifications = [{
    id: 7,
    level: "error",
    title: "Metadata scrape finished with issues",
    message: "Matched: 2\nFailed: 1\n\nIssues:\n• Example Movie — request timed out",
    created_at_ms: Date.now(),
  }];
  await dom.window.refreshNotifications();
  const notificationRow = document.querySelector('[data-open-notification="server-7"]');
  if (!notificationRow) {
    failures.push("Expected persistent server notifications to render on the Notifications page.");
  } else {
    notificationRow.click();
    if (document.getElementById("notificationModalBackdrop").classList.contains("d-none")) {
      failures.push("Expected clicking a notification preview to open its full-detail modal.");
    }
    if (!document.getElementById("notificationModalBody").textContent.includes("request timed out")) {
      failures.push("Expected the notification modal to contain the full scrape error report.");
    }
    const notificationModalMaxWidth = dom.window.getComputedStyle(document.querySelector(".notification-modal-box")).maxWidth;
    if (notificationModalMaxWidth !== "1360px") {
      failures.push(`Expected the notification modal to have a 1360px maximum width, got ${notificationModalMaxWidth || "no value"}.`);
    }
    document.getElementById("notificationModalCopy").click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    const expectedCopiedText = [
      document.getElementById("notificationModalTitle").textContent,
      document.getElementById("notificationModalMeta").textContent,
      document.getElementById("notificationModalBody").textContent,
    ].join("\n\n");
    if (copiedText !== expectedCopiedText) {
      failures.push("Expected Copy notification to include the title, metadata, and full notification details.");
    }
  }

  // Group re-scrape regression: the show action must include episodes from
  // every season, while the season action must stay scoped to only that
  // season. Both reuse the real rescrape_entry command sequentially.
  testLibraryEntries = [
    { entry_key: "s1e1", kind: "episode", title: "Pilot", relative_path: "Shows/Test/S01/E01.mkv", genres: [], cast: [], show_title: "Test Show", season: 1, episode: 1, like_count: 0 },
    { entry_key: "s1e2", kind: "episode", title: "Second", relative_path: "Shows/Test/S01/E02.mkv", genres: [], cast: [], show_title: "Test Show", season: 1, episode: 2, like_count: 0 },
    { entry_key: "s2e1", kind: "episode", title: "Return", relative_path: "Shows/Test/S02/E01.mkv", genres: [], cast: [], show_title: "Test Show", season: 2, episode: 1, like_count: 0 },
  ];
  dom.window.eval(`libraryEntries = ${JSON.stringify(testLibraryEntries)}; browsePath = { kind: "show", show: "Test Show" }; renderMediaTab();`);
  invokeCalls.length = 0;
  document.getElementById("rescrapeShowBtn")?.click();
  await new Promise((resolve) => setTimeout(resolve, 50));
  const showKeys = invokeCalls.filter(call => call.command === "rescrape_entry").map(call => call.args.entryKey);
  if (JSON.stringify(showKeys) !== JSON.stringify(["s1e1", "s1e2", "s2e1"])) {
    failures.push(`Expected show re-scrape to process every season in order, got ${showKeys.join(", ")}.`);
  }

  dom.window.eval(`browsePath = { kind: "season", show: "Test Show", season: 1 }; renderBrowse();`);
  invokeCalls.length = 0;
  document.getElementById("rescrapeSeasonBtn")?.click();
  await new Promise((resolve) => setTimeout(resolve, 50));
  const seasonKeys = invokeCalls.filter(call => call.command === "rescrape_entry").map(call => call.args.entryKey);
  if (JSON.stringify(seasonKeys) !== JSON.stringify(["s1e1", "s1e2"])) {
    failures.push(`Expected season re-scrape to process only that season, got ${seasonKeys.join(", ")}.`);
  }

  dom.window.close();

  if (failures.length > 0) {
    console.error("FAIL: boot_order.test.js\n  " + failures.join("\n  "));
    process.exitCode = 1;
  } else {
    console.log("PASS: boot_order.test.js -- boot() reached the dashboard with no ReferenceError, even with invoke() resolving instantly.");
  }
}

main().catch((err) => {
  console.error("FAIL: boot_order.test.js threw while running the test itself:", err);
  process.exitCode = 1;
});
