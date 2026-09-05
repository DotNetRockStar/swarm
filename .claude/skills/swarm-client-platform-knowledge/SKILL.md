---
name: swarm-client-platform-knowledge
description: Use when building, changing, or reasoning about any SWARM TV client (clients/tv-android today, clients/tv-roku, or a future platform) - the portable spec of what every client must do, the http/HLS contract it talks to the media server over, the UX rules that must hold on any 10-foot platform, the state/navigation model, and per-platform implementation notes/workarounds/lessons learned. Read this before starting a new client platform or before making a cross-cutting change to an existing one.
---

# SWARM client platform knowledge

SWARM ships one product experience across multiple TV platforms. This skill is the
portable spec that makes each new platform client faster to build than the last one — it
captures what a client must *do* (features, protocol, UX contract, state model) separately
from how any one platform happens to implement it. `clients/tv-android` (Fire TV, Kotlin +
Compose for TV) is the original, most complete client and the source every reference file
here was distilled from. `clients/tv-roku` (Roku, BrightScript + SceneGraph) is the second.

**This skill is a living document.** Every time a client platform teaches the project
something new — a platform limitation, a workaround, an API quirk discovered the hard way,
a UX rule that turned out to matter — update the relevant reference file here, not just the
one client's own code comments. The whole point is that the *third* client (and the
fourth) should never have to rediscover something the first two already learned.

For the companion rule that changes here must be cross-checked against every client, see
[[swarm-cross-client-parity]]. That skill owns the day-to-day "did I just make Fire TV and
Roku diverge" discipline; this skill owns the reference material both skills point at.

## How to use this during a task

1. **Adding a feature/behavior to an existing client, or fixing a bug in one**: read
   `references/feature-inventory.md` to see the feature's current status across all
   clients, then `references/ux-rules.md` / `references/state-model.md` for the behavior
   contract it must match. After the change, update the inventory row and follow
   [[swarm-cross-client-parity]].
2. **Starting a new platform client from scratch**: read every file in `references/` in
   order (contract → UX rules → state model → an existing platform-notes file as a worked
   example), then write your own `references/platform-notes/<platform>.md` as you go —
   don't wait until the end. Use `references/feature-inventory.md` as your build checklist,
   working from the top (core: pairing, catalog browse, playback) down.
3. **Discovering something new mid-task** (a platform API limitation, a server behavior
   that wasn't documented, a UX rule you had to invent to make the platform work): stop and
   add it to the right reference file **before** moving on. A discovery that only lives in
   your head or in a PR description is not captured.

## Reference files

- **`references/feature-inventory.md`** — the feature-parity table: every user-visible
  SWARM behavior, tagged Complete / Partial / Not implemented / Platform-specific deviation
  per client. The single source of truth for "what does Roku still need."
- **`references/http-client-contract.md`** — the transport-agnostic client↔server
  contract: every endpoint, payload shape, status code, retry policy, and timeout a client
  must implement, independent of whether the transport underneath is QUIC or plain HTTPS.
- **`references/ux-rules.md`** — the palette, focus/D-pad rules, loading/error/empty-state
  conventions, and layout rules that must hold on any TV platform, generalized from
  Fire TV's own `tv-client-ui-conventions` skill (that skill stays the authority for
  Compose-specific implementation detail; this file is the platform-neutral version of the
  same rules).
- **`references/state-model.md`** — the "screens are states" navigation model, the
  fingerprint-vs-entry-key identity rules, watch-state/watchlist/like semantics, kid-mode
  enforcement, and the playback-session-reservation discipline every client must reproduce.
- **`references/platform-notes/fire-tv.md`**, **`references/platform-notes/roku.md`** —
  per-platform mapping tables (SDK primitive ↔ portable concept), hard platform
  constraints, workarounds, and lessons learned. Add a new file here for each future
  platform (Apple TV, Android TV/Google TV, LG webOS, Samsung Tizen, …).

## Why a spec file per concern, not one giant document

Early drafts tried to keep this as one file. It didn't survive contact with a second
platform: the HTTP contract, the UX rules, and the state model change at very different
rates and are consulted by different phases of work (contract while building the network
layer, UX rules while building screens, state model throughout). Splitting them lets each
stay small enough to actually read before starting, and lets `references/platform-notes/`
grow by one file per platform without touching the shared files at all.
