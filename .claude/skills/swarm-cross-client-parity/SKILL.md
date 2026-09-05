---
name: swarm-cross-client-parity
description: Use whenever you change any client application (clients/tv-android, clients/tv-roku, or a future client) - a feature, bug fix, UX change, API change, playback behavior, preference, or anything user-visible - or any server API/behavior a client depends on. Establishes the rule that every such change must be evaluated against every other supported client, not just the one you're touching. Maintain the client-parity checklist in this file and keep swarm-client-platform-knowledge's feature-inventory.md current together.
---

# Cross-client parity

SWARM ships the same product on multiple TV platforms. **Any change to one client, or to a
server API/behavior a client depends on, must trigger an explicit check of every other
supported client** — Fire TV, Roku, and whatever comes after — before the change is
considered done. This is not "remember to port it eventually." It is a required step of
the change itself, the same way updating a test is a required step of a code change.

The reference material this skill's checks are evaluated against lives in
[[swarm-client-platform-knowledge]] (the portable contract/UX/state-model spec) and its
`references/feature-inventory.md` (the current status table). This skill is the *process*;
that skill is the *content*.

## The rule, precisely

Before you consider any of the following changes finished, answer explicitly — in the PR
description or commit message, not just in your own head — "does this need an equivalent
change on the other client(s), and did I make it (or deliberately not make it, and record
why)?":

- A new user-visible **feature** on one client
- A **bug fix** that changes observable behavior (not a pure internal refactor)
- A **UX change** (new screen, changed layout, changed interaction, changed copy/wording
  that isn't a typo fix)
- An **API change** on the server that any client calls (new field, new endpoint, changed
  status code, changed payload shape, changed auth requirement)
- A **playback behavior change** (transcode decision logic, retry/timeout tuning, resume
  semantics, capability negotiation)
- A **preference/setting change** (new setting, changed default, changed persistence
  semantics)
- Any other **behavior change** a user could notice by using two different clients against
  the same server and comparing

A change that is purely internal to one client's implementation (refactor with no
observable behavior difference, a platform-specific performance fix, a dependency bump with
no behavior change) does **not** require this check — but if you're unsure whether a change
is "purely internal," treat it as in-scope. Silent drift is exactly the failure mode this
skill exists to prevent, and it's cheaper to over-check than to discover a drift months
later.

## The four possible outcomes for any one change, per other client

1. **Ported together** — the equivalent change is made on every other client in the same
   piece of work. Preferred whenever the change is small enough or the other client's
   corresponding code is close enough that splitting the work would just add coordination
   overhead.
2. **Tracked as a gap** — the other client doesn't have the equivalent yet for an unrelated
   reason (that whole feature area isn't built yet, it's a big enough change to warrant its
   own follow-up). Update `feature-inventory.md`'s status cell to **Not implemented** or
   **Partial** for that client/feature, and open (or point to) a tracking issue if the gap
   is significant. This is fine — it's an honest, visible gap, not silent drift.
3. **Intentional platform-specific deviation** — the other client genuinely should behave
   differently (a platform capability doesn't exist there, a platform convention overrides
   the shared one, a security/trust-model difference like Roku's relay vs. Fire TV's
   hole-punch). Add or update a row in `feature-inventory.md`'s deviation table **with a
   real reason** — "ran out of time" or "didn't think about it" are never valid deviation
   reasons; those are outcome 2 (a gap) mislabeled.
4. **Not applicable** — the change is genuinely platform-specific with no shared-product
   concept behind it at all (e.g., a Fire TV `AndroidManifest.xml` permission fix). No
   inventory update needed, but it's worth a moment's thought to confirm this is really true
   before skipping the other three outcomes.

Every one of these outcomes ends with `feature-inventory.md` reflecting reality. An
outcome-2 or outcome-3 that never gets recorded is indistinguishable from a change nobody
ever thought about the parity impact of — which is the exact failure this skill exists to
prevent. Recording it is not optional busywork.

## Checklist to run through when you're about to call a change done

- [ ] Did I identify every client this repo currently supports? (Check
      `clients/` — don't rely on memory; a new client can have been added since you last
      looked.)
- [ ] For each *other* client, which of the four outcomes applies to this change?
- [ ] If "ported together" or "tracked as a gap": is `feature-inventory.md` updated in this
      same change?
- [ ] If "intentional deviation": is the deviation table row present with a real reason,
      not just a status flip?
- [ ] If this touched a server API/behavior: does [[swarm-client-platform-knowledge]]'s
      `http-client-contract.md` (or `state-model.md`/`ux-rules.md`, whichever applies) still
      accurately describe the contract? A server change that isn't reflected there will mislead
      the *next* platform client's author, not just today's readers.
- [ ] If this was itself a new discovery about a platform limitation, workaround, or lesson
      learned (not just a routine feature port): did I add it to the relevant
      `references/platform-notes/<platform>.md` file? Don't let a hard-won discovery live
      only in a PR description or your own memory.
- [ ] Does this repo's own `swarm-verify-before-commit` sequence still apply, and did it
      run clean, for every client/crate this change touched?

## Common failure modes this skill exists to prevent

- **"I fixed it on the client I was already looking at and moved on."** The most common
  drift vector. The checklist above exists specifically to make the "and what about the
  other client" question unskippable.
- **A server API change ships with only one client updated to use it**, and the *contract
  document* isn't updated either — so the next person building against that API (a future
  platform, or the other existing client catching up later) has no accurate spec to read
  and has to re-derive the change from server source instead.
- **A deviation becomes permanent by default** rather than by decision — a gap that was
  meant to be temporary ("Roku doesn't have kid mode yet, will add it after core playback
  ships") silently turns into a deviation nobody ever revisits because it was never written
  down as a gap with an owner/tracking issue in the first place.
- **A UX rule changes on one client** (a new palette color, a changed toast duration, a
  changed focus behavior) **without updating `ux-rules.md`** — so the *next* platform port
  copies the now-stale documented rule instead of the actual current behavior.

## Relationship to other skills

- [[swarm-client-platform-knowledge]] — the content this skill's checks are evaluated
  against, and the place discoveries get written down.
- `.claude/skills/swarm-verify-before-commit` — this skill adds a parity check *on top of*
  that verification sequence, not instead of it; both apply to any commit touching a
  client.
- `.claude/skills/swarm-contract-fixtures` — for the specific mechanics of mirroring a Rust
  wire type into a client language faithfully (verified against real `serde_json` output,
  not guessed). Use it whenever an API-change outcome above involves a shared wire type.
