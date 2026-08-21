---
name: media-server-dashboard-ui
description: Use when building or changing anything in the Tauri media server's web dashboard (apps/server/ui — index.html, app.js, details.js, swarm.js, notifications.js, media.js, ai.js, style.css). Covers the no-framework vanilla-JS conventions (invoke(), esc(), showToast()), the shared SwarmBackground/SwarmAccent-style CSS palette, the tab system, script-load-order pitfalls, and the pattern for adding a new panel like "Client errors." For the Fire TV client's UI conventions (a separate, Kotlin/Compose surface sharing the same palette), see tv-client-ui-conventions.
---

# Media server dashboard UI conventions

`apps/server/ui` is plain HTML/CSS/vanilla JS served into a Tauri
webview — no React/Vue/build step. Read this before adding or changing
anything here; several of these are silent failure modes that only show
up under real timing, not in a quick look at the code.

## No framework: `invoke()`, real DOM, real `<script>` tags

Every backend call goes through `window.__TAURI__.core.invoke("cmd", {...})`
(aliased to `invoke` at the top of `app.js`) — a Tauri IPC command
defined on the Rust side, not a REST call. There's no component
framework: screens are built by setting `.innerHTML` on a container
element with a template string, then wiring `addEventListener` calls
onto the elements that string just created. Follow this same shape for
new panels rather than introducing a different pattern (a template
engine, a framework) for one feature.

Six script files load in a fixed order (`index.html`'s bottom):
`app.js` → `details.js` → `swarm.js` → `notifications.js` → `media.js`
→ `ai.js`. `app.js` owns boot/tabs/toast/shared helpers; the other five
each own one tab's content and are expected to define whatever function
`app.js`'s `showTab()` dispatches to for that tab (`refreshDetails`,
`refreshSwarm`, `refreshNotifications`, `refreshMedia`, `refreshAi`).
The `about` tab is the one exception — see "Tabs" below.

**Real bug, found live, twice**: calling any of those tab-refresh
functions (or anything else defined in a later-loading file) from code
that runs unconditionally at a file's own top level is a genuine race,
not a one-off — each classic `<script>` gets a microtask checkpoint
after it runs, and if an `invoke()` round trip resolves fast enough,
the continuation can call a function that hasn't been defined yet
(surfaces as e.g. `refreshNotificationBadge is not defined`, easy to
misdiagnose as a settings/persistence bug since it's caught by the same
top-level `try/catch` that also handles "onboarding not finished").
Fix: gate any such call behind `document.addEventListener("DOMContentLoaded", ...)`,
which only fires after every classic script in the document has
finished executing. Regression test: `apps/server/ui/test/boot_order.test.js`
— extend it if you add a new cross-file call at boot.

## Avoid initial-state-flash: reveal the body only once, deliberately

`index.html` has an inline `<style>body { visibility: hidden; }</style>`
in `<head>`, deliberately *not* moved into `style.css`. The three
top-level views (`onboardFolderView`/`onboardSwarmView`/`dashView`) all
start `class="d-none"` in the markup, but `.d-none` is defined in the
external `style.css`, which loads over its own separate request — on a
slow enough load, the raw un-hidden HTML could briefly paint before
`style.css` caught up, flashing the onboarding card even on an install
that was always headed straight to the dashboard. The inline style
parses as part of the very first HTML chunk, before any external file
round trip, so it can't race. `app.js`'s `show(id)` is the one place
that reveals `document.body.style.visibility = "visible"`, called only
once `boot()` has actually resolved which view to display. (The Fire TV
client has the same class of bug, fixed the same way conceptually via a
real `Loading` state — see `tv-client-ui-conventions`.)

If you add a new top-level view that can be chosen at boot, route its
reveal through the same `show()` call rather than unhiding it by hand —
that's the one chokepoint this guarantee depends on.

## `esc()`: mandatory on every interpolated value in a template string

`app.js`'s `esc(v)` HTML-escapes a value before it's spliced into an
`.innerHTML` template string. **Every** value that isn't a literal —
user input, a device name, an error message, anything from `invoke()`'s
response — must go through `esc()` at the point it's interpolated, no
exceptions, since this is the app's only XSS boundary (there's no
framework auto-escaping on your behalf here). Grep the file you're
editing for other `${esc(...)}` call sites and match that pattern
exactly; a bare `${value}` in a template string is a real vulnerability
here, not just a style nit.

## `showToast()`: every catch block reports here, never silently

`showToast(message, type, opts)` (`type`: `"success"` | `"warning"` |
`"error"`, default duration 4.5s / 7s for errors) is the single
notification surface for the whole app — no scattered inline status
text. Every `try { await invoke(...) } catch (err) { ... }` in this
codebase routes `err` to `showToast(String(err), "error")` in the catch
block; a new call site that swallows an error without a toast is a
regression, not a simplification. The one deliberate exception is a
best-effort background poll (`refreshNotificationBadge`'s `catch {}`)
where a transient failure every 30s isn't worth a toast — comment why
if you add another one of these, don't let a silent catch look
accidental.

## Shared palette: keep in sync with the Fire TV client

`style.css`'s `:root` variables (`--bg: #101828`, `--surface: #151f32`,
`--surface-muted: #1f2a44`, `--border: #31405f`, `--accent: #00c2ff`,
`--green: #34d399`, `--text: #ecf6ff`, `--muted: #9fb0c9`) are the
canonical source the Fire TV client's `SwarmTvTheme` was ported *from*
— same hex values on both sides deliberately, so the two apps read as
one product (see `tv-client-ui-conventions`). Reference these
`var(--...)` tokens, never a hand-picked hex literal; if you introduce
a genuinely new semantic color, add it here first and port it to the
Kotlin side too rather than letting one app silently drift off-palette.

## Structural building blocks — reuse before inventing new ones

- `.card` / `.card-head` (+ `.card-head-actions`) — the standard
  bordered content block with a title-row-plus-actions header. Almost
  every panel is one or more of these.
- `.grid` + `stat(label, value, mono)` (`app.js` helper) — the
  auto-fitting label/value tile grid used for status summaries.
- `.row` — a flex row of inputs/buttons for an inline form (used for
  "add a root", "join a swarm", etc.).
- `.muted`, `.mono`, `.d-none`, `button.secondary`, `button.danger` —
  standing utility classes; reach for these before writing new CSS.
- Icons are Bootstrap Icons (`<i class="bi bi-icon-name">`), always
  paired with the button/label text, never icon-only for a primary
  action.
- Tabs: `TABS` array in `app.js` (currently `about`, `details`, `swarm`,
  `notifications`, `media`, `ai`, in on-screen left-to-right order —
  `about` is deliberately leftmost and is also the tab `enterDashboard()`
  opens on at boot, since it's the first thing every user should see),
  `tabPanel-<name>` / `tabBtn-<name>` id convention, `showTab(name)`
  toggles `.d-none`/`.tab-active` and dispatches to that tab's
  `refresh*()`. Adding a tab means adding to `TABS`, adding the matching
  `tabPanel-`/`tabBtn-` markup, and adding the
  `if (name === "...") refresh...()` dispatch line. `about` is the one
  tab with no dispatch line — its content is static, nothing to refresh.
- A small red count bubble on a tab button (`.badge-count`, see
  `#notificationBadge` on the Notifications tab) is this app's pattern
  for "there's something here you haven't looked at yet" — paired with a
  `setInterval`-polled `refresh*Badge()` function (30s interval is the
  existing standard) rather than a push mechanism, since there's no
  websocket/event channel wired up for this yet.

## Adding a new report/feed-style panel: follow the "Client errors" shape

The Notifications tab's "Client errors" panel (`notifications.js`'s
`loadClientErrors`,
`style.css`'s `.client-error-row`/`.client-error-meta`/`.client-error-context`)
is the reference shape for "a list of server-recorded events a human
should triage and dismiss": a header with a live count and a
conditionally-shown "Clear all" (`.d-none`-toggled based on whether
there's anything to clear), each row showing a primary message plus a
`flex-wrap`ped meta line of small icon+text tags
(`<i class="bi bi-...">` + value), an optional secondary context block,
and a per-row delete button (`data-delete-*` attribute + a single
`querySelectorAll(...).forEach(addEventListener)` pass after the
`innerHTML` render, the standard way this codebase wires up
dynamically-created rows). Copy this shape for any new "list of things
from the backend that accumulate and get dismissed" panel rather than
designing a new list layout from scratch. This panel moved here from the
Swarm tab once its badge grew into its own "things to look at" surface
distinct from swarm membership/roster management — a similar future
panel might belong on this tab too rather than wherever its data
technically originates.

## Adding a "row of things" list: give each row a real delineated card

`.media-root-row` (Details tab, `details.js`'s `refreshMediaRoots`) is
the reference shape for a *simpler* list than "Client errors" above —
no per-row expand state, no meta-tag line, just "one thing plus a
delete/remove action" — but still needs its own bordered/backgrounded
row (padding, `border-radius`, `margin-bottom`) rather than bare
adjacent `.row` divs with nothing marking where one entry ends and the
next begins. Real feedback this fixed: a plain stack of `.row` divs (no
border, no background, no margin) read as "too close together, no
delineation" once there was more than one entry — the fix is the same
"give it a card" shape `.client-error-row` already uses, scaled down.
