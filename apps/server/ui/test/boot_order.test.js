#!/usr/bin/env node
//
// Regression test for a real bug found live *twice*: app.js's boot()
// (via enterDashboard() -> showTab("details")) calls functions defined in
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

function invokeStub(command) {
  switch (command) {
    case "get_settings":
      // Non-empty media_roots is what sends boot() down the enterDashboard()
      // path -- an empty/missing one just shows onboarding, never reaching
      // any of the functions this test exists to guard.
      return { media_roots: [{ label: "test", path: "/tmp/swarm-boot-order-test" }], has_tmdb_key: false, tmdb_api_key: null };
    case "get_status":
      return {
        fingerprint: "test-fingerprint",
        media_roots: ["test: /tmp/swarm-boot-order-test"],
        listen_addr: "0.0.0.0:8543",
        entry_count: 0,
        thumbprint: "0".repeat(32),
        streaming_upload_budget_bps: 1000000,
        active_playback_sessions: 0,
        scanning: false,
      };
    case "list_entries":
    case "list_media_roots":
    case "list_categories":
    case "list_client_errors":
      return [];
    case "client_error_count":
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
        core: { invoke: (command) => Promise.resolve(invokeStub(command)) },
        event: { listen: () => Promise.resolve(() => {}) },
      };
      window.IntersectionObserver = FakeIntersectionObserver;
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
  // Companion regression: index.html's inline `body { visibility: hidden }`
  // guard (a flash-of-onboarding fix, same "wrong view painted first" family
  // as this file's main bug) only ever gets lifted by show() -- if body
  // stayed hidden, the user would see nothing at all rather than the wrong
  // view, but it's the same "did boot() actually finish deciding what to
  // show" question this whole file is about.
  if (dom.window.getComputedStyle(document.body).visibility !== "visible") {
    failures.push("Expected document.body to be revealed (visibility: visible) after boot, but it was still hidden.");
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
