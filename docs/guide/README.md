# SWARM System Guide (interactive)

`index.html` is a single, self-contained, audience-switching walkthrough of the
whole system — devices, media server, STUN/rendezvous server, LAN connections,
security in fine detail, technology choices, protocol/data flows, deployment,
and the full testing story (unit, UAT, integration).

## How to read it

Open `index.html` in any browser — no build step, no server, no network. Use the
**User / Engineer** switch in the header:

- **User** — what each part does, what you set up, what "secure" means in plain
  terms.
- **Engineer** — everything in the User view plus wire protocols, sequence and
  topology diagrams, trust boundaries, data schemas, technology rationale, and
  the full test strategy.

Your choice is remembered per browser (`localStorage`). With JavaScript
disabled, every section is shown and the switch is inert.

## Keeping it honest

The guide restates facts owned by `README.md`, `docs/PROTOCOL.md`, the
client/deploy READMEs, and `scripts/tests/TV_TESTING.md`. `tests/docs/`
(`cargo test -p swarm-docs`, also part of `cargo test --workspace`) fails if the
guide loses its audience switch, grows a broken in-page link, gains an external
CDN dependency, or drifts from the HLS ladder / env-var / security facts in
those sources. Update the guide in the same change as the thing it describes.
