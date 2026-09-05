# SWARM — Roku client

A native Roku client for SWARM, built with BrighterScript + SceneGraph. Issue #54's
requested Fire TV counterpart — see [[swarm-client-platform-knowledge]] (the portable spec
this client implements against) and [[swarm-cross-client-parity]] (the rule that any change
here or on Fire TV gets evaluated against the other) for the two Claude skills that go with
this work, and `references/feature-inventory.md` for the exact, row-by-row status of every
feature against Fire TV.

## What this is (and isn't) — read this before assuming full parity

This ships a **real, compiling, working vertical slice**: discover a server on the LAN,
pair with it, browse a merged catalog (Movies / Shows / Music shelves), open a movie's
detail screen, and play video or music with save/resume — the whole core loop end to end,
not a UI mockup with stubbed networking. It is **deliberately not yet full parity** with
the ~20-screen Fire TV client. `feature-inventory.md` is the accurate source of truth for
what's implemented vs. deferred; the short version:

- **LAN-first, one server at a time.** No STUN/swarm membership, no multi-server roster, no
  off-LAN reach yet (the server already issues TLS for this — see below — but the relay
  that would let Roku *reach* an off-LAN server hasn't been built).
- **Shelves are flat and capped at 10 items**, no genre sub-shelves, search, filter rail,
  "Browse All" grid, or Continue Watching/Watchlist rows.
- **Selecting a show or artist plays its first episode/track directly** — there's no
  season/episode or album/track browsing screen yet.
- **The player is core-only**: negotiate, play, periodic resume-position save, release the
  session on exit. No pause overlay, skip-intro markers, subtitle/audio track picker, "up
  next" prompt, or a dedicated music screen (music plays through the same bare player).
- **Per-item state (watch position, likes, watchlist) is LRU-capped**, not unbounded like
  Fire TV's storage — a deliberate, documented platform deviation (Roku's registry is
  ~32KB/section; see `platform-notes/roku.md`).

## Architecture, in one paragraph

One `AppState` node (`components/app/AppState.xml`) mirrors Fire TV's `UiState` — a
`currentScreen`/`screenData` pair plus a one-level `previousScreen` back-chain — and
`MainScene` observes it, mounting exactly one screen component at a time. Every network
call runs in a `Task` node (`components/tasks/`) so the render thread never blocks. Screens
outside `CatalogScreen`'s shelves use manual `onKeyEvent` focus routing (a `SwarmButton`
widget with an explicit `isFocused` field the owning screen drives) rather than native
SceneGraph focus traversal — deliberate, given the simple list/button layouts involved; see
`references/ux-rules.md`'s two-focus-idiom note. `Registry.bs` wraps `roRegistrySection` for
all durable state; `Http.bs` wraps `roUrlTransfer` for all network calls.

## Build

```bash
cd clients/tv-roku
npm install
npm run build          # compiles + type-checks via bsc, no package
```

`npm run build` runs `bsc --project bsconfig.json` — every source file is compiled and
cross-checked (including component XML interfaces and `CreateObject("roSGNode", "Literal")`
calls against known components) on every change. This is the fast local dev-loop check;
run it after any edit.

## Deploy to a real device

```bash
./scripts/deploy_roku.sh                  # scans the LAN, lists Rokus, prompts for a target
./scripts/deploy_roku.sh 192.168.0.150    # or target an IP directly
./scripts/deploy_roku.sh -l               # also launches the channel after install
```

First time only, on the device itself: enable Developer Mode with the physical remote —
**Home ×3, Up ×2, Right, Left, Right, Left, Right** — and set a developer password when
prompted. That password is what the deploy script asks for (or set `SWARM_ROKU_PASSWORD`).

## What has **not** been verified on real hardware

Everything above compiles cleanly and was reasoned through carefully, but this project had
no access to real Roku hardware or an emulator — every claim below is a real, open risk,
not a formality:

1. **`roDatagramSocket` mDNS discovery** (`components/tasks/MdnsDiscoveryTask.bs`) — the
   exact multicast join/send/receive call shape is the least-confident code in this client.
   The DNS *parsing* logic it feeds into (`source/Mdns.bs`) was independently verified
   against a hand-built RFC 1035-shaped packet using an equivalent Python implementation
   (see the commit that introduced it) and is not in question; the socket I/O around it is.
   If discovery doesn't find servers on real hardware, start here.
2. **fMP4/CMAF HLS with an `EVENT` playlist** — the server's HLS output format. Confirm the
   target Roku OS's `Video` node handles both before assuming HLS playback works.
3. **`HttpHeaders` reaching HLS segment requests**, not just the master playlist — a bearer
   token that only reaches the manifest would break HLS playback with a misleading
   "it played the first few seconds then failed" signal.
4. **`ContentNode.PlayStart`/`Video` state transitions** used for direct-play resume and
   play/pause toggling.
5. **The pinned-TLS HTTP path** (`Swarm.Http.GetPinned`/`PostJsonPinned`, backed by
   `HTTPCertificatesFile`) — implemented, wired to receive `httpCaPem` at pairing time, but
   not yet the path actually used end-to-end (v1 defaults to plain HTTP throughout). Prove
   this with a throwaway CA/leaf and a plain `roUrlTransfer` GET before building the relay
   work on top of it.

See `platform-notes/roku.md`'s "Open items" checklist — update both that file and this one
as each item gets resolved.

## Server-side prerequisite

Nothing extra to configure — `apps/server/src/http_media.rs`'s plain-HTTP surface
(`:8546` by default) was built specifically for this client and requires no server changes
to use. The TLS listener (`:8547`) and CA distribution are also already live server-side;
this client just doesn't route through them yet (see point 5 above).
