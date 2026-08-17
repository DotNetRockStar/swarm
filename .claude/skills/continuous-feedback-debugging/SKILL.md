---
name: continuous-feedback-debugging
description: Use when a real system (real device, real server, real network — not a mock/emulator/unit test) fails silently, intermittently, or in a way that "looks fine" in code review, and the only way forward is instrument-rebuild-redeploy-observe, repeated as many times as it takes. Recognize requests like "keep debugging until it actually works," "test this for real," "don't stop at the first fix," or a bug report that survived compile/test/lint clean. Also use to explain or repeat the specific technique that found three chained real-hardware bugs (dead D-pad focus, an API-33-only NoSuchMethodError, an IPv6/IPv4 loopback mismatch) in one continuous SWARM Fire TV debugging session — see swarm-real-device-debugging for those bugs' full writeups.
---

# The continuous feedback debugging loop

This is the *process*, not the bugs. For this project's specific
real-hardware findings (namespace mismatches, off-screen D-pad focus,
missing-initial-focus, an API-level NoSuchMethodError, an IPv6/IPv4
loopback bind mismatch), see `swarm-real-device-debugging` — this
skill is about the general loop that found all of them, written so
the same approach reproduces on a *different* bug, in a *different*
codebase, on a *different* kind of real hardware.

## When to reach for this

Compile succeeds, lint is clean, unit tests pass, and the thing still
doesn't work when a real human presses a real button on a real device
talking to a real server over a real network. That gap exists because
none of those checks execute the real runtime, the real OS version,
the real classloader, the real network stack, or the real timing —
only a live run does. Reach for this loop specifically when:

- The failure is **silent** — swallowed by a `try/catch`, a
  `runCatching { }.getOrNull()`, a "mark unreachable and move on"
  fail-open design, or a UI that just... doesn't respond.
- The failure **can't be reproduced by reading the code** — it depends
  on real hardware's Android API level, a real machine's IPv6/IPv4
  resolution, a real VPN being active, real timing between two real
  processes.
- A first fix makes the *symptom* change but the underlying goal still
  isn't achieved — that's a sign there's another bug stacked
  underneath, not a sign the work is done.

## The loop

1. **Reproduce cleanly, the same way every time.** Nail down an exact,
   minimal, repeatable sequence of real actions that gets you to the
   failure — not "it doesn't work," but the specific steps, starting
   from a specific, known-clean starting state. Inconsistent repro
   steps make every later log read ambiguous.
2. **If the failure is silent, make it loud — at the exact point it's
   being swallowed, nowhere else.** Find every `catch`/`runCatching`/
   `except` between the real failure and where you're observing
   "nothing happened," and add real diagnostic output at each one
   (`printStackTrace()`, a real log line, whatever the platform
   surfaces to a tool you can read afterward). Resist adding broad
   logging everywhere "just in case" — targeted beats broad, because
   broad logging buries the one line that matters in noise you'll
   have to read past under time pressure.
3. **Rebuild and redeploy for real.** Not a hot-reload, not a
   from-memory guess about what the code does now — an actual fresh
   build pushed to the actual real target.
4. **Re-run the exact same repro from step 1.** Same sequence, same
   starting state. If the starting state can't trivially be restored
   (e.g. an auth session that doesn't survive a reinstall), rebuild
   *that* too as part of the repro, every single time — don't let
   setup drift between attempts become a confound.
5. **Capture the real output and read it before theorizing.** Pull
   the actual log lines, the actual stack trace, the actual screen
   state (screenshot, not "it should show X"). A hypothesis formed
   before looking at real evidence is a guess; write it down if you
   want, but treat it as disproven until the evidence says otherwise.
6. **Form exactly one hypothesis the evidence actually supports, then
   check it independently before touching code.** If the evidence is
   ambiguous ("might be the client, might be the server"), reach for
   an independent ground truth — a direct API/DB query, a second log
   source, whatever is authoritative and separate from the thing
   you're debugging — rather than adding a second layer of
   instrumentation to the same suspect component. Two independent
   signals agreeing is real confirmation; one component's logs
   agreeing with themselves is not.
7. **Apply the smallest fix that addresses the confirmed root cause.**
   Before writing it, check whether the same mistake exists elsewhere
   (a repo-wide grep for the same pattern) — that tells you whether
   you're fixing an isolated slip or a systemic habit, and shapes how
   much you should trust "just this one" as the whole fix.
8. **Rebuild, redeploy, re-run the exact same repro again.**
9. **Verify with evidence stronger than "no error observed."** No
   error is a weak signal — plenty of bugs are silent. Prefer positive
   proof: a log line showing the exact expected outcome (a count, a
   status, a specific value), a screenshot showing the expected visual
   state, and — for anything with state that changes over time (video
   playing, a counter incrementing, a connection staying alive) — a
   *second* observation moments later showing it's still correct, not
   just correct in one frozen instant.
10. **If verification reveals a *different* failure than before, that's
    progress — keep looping.** A changed symptom means the layer you
    just fixed really was broken and is now behind you; there's
    another real bug in front of you. Getting further before failing
    differently, `pass 1 -> fail differently at pass 2 -> fail
    differently at pass 3`, is exactly what fixing a chain of stacked
    real bugs looks like from the inside — don't mistake it for "my
    fix didn't work." A fix that produces the *identical* failure is
    the one that should worry you.
11. **Stop only when the original goal is verified, not when the
    original symptom is gone.** "The button responds now" is not the
    goal if the goal was "browse and play a real file" — keep going
    until the thing the user actually asked for is independently
    confirmed to work.
12. **Once real, close the loop like real work:** run the project's
    full verification suite (not just the one thing you touched) before
    calling it done, decide what diagnostic logging is worth keeping
    permanently versus reverting as scaffolding (a `catch` block that
    silently hid a real bug once will hide the next one too — logging
    that closes a genuine blind spot earns its place in the codebase;
    a one-off print statement that answered a single question doesn't),
    and write down what you found somewhere durable (a project skill,
    a doc, a commit message that explains *why*) so the next person —
    human or agent — doesn't have to re-run the whole loop from
    scratch for the same bug.

## Supporting techniques that make the loop actually work

- **Cross-reference independent log sources by real wall-clock
  timestamp**, not just each source's internal ordering. Two
  processes' clocks are usually close enough to compare directly; a
  gap between "client gave up" and "server finished" that's larger
  than it should be is often the clue itself (a race, a connection
  that succeeds *after* the caller already stopped waiting for it).
- **Use the system's own legitimate recovery paths to regain test
  access, rather than fabricating state.** Lost a test account's
  password mid-session here — used the app's real password-reset flow
  (reading the reset link from the server's own log, since outbound
  email wasn't configured for local testing) rather than hand-editing
  the database. Exercises a real path instead of a shortcut, and
  avoids leaving the database in a state nothing else would produce.
- **When driving a UI programmatically (adb, browser automation,
  etc.), verify visually after every step that could plausibly have
  gone somewhere unexpected — don't chain blind actions.** A batched
  sequence that lands wrong is far cheaper to catch one step later
  than several steps later, especially anywhere a wrong input could
  be destructive (a delete, a backspace over data you need).
- **Recognize when you're about to add a second guess on top of an
  unconfirmed first one, and stop.** If a timing/race theory feels
  plausible but you haven't actually confirmed the timing, that's the
  moment to add instrumentation and get a real answer instead of
  designing a fix around a story that might be wrong.
- **A negative result from added instrumentation is still a result.**
  "The exception handler I just added never fired" rules out an entire
  branch of the call graph and tells you exactly where to add the next
  probe — it's not a wasted step, it's the loop working correctly.

## Anti-patterns

- Fixing the first plausible-looking cause without evidence it's the
  *actual* cause, then declaring victory when the symptom merely
  changes shape.
- Broad, permanent logging sprayed everywhere "to be safe," which
  buries the one signal that matters and rarely gets cleaned up.
- Treating "it didn't crash" or "no error in the last few lines of
  output" as proof of success, instead of checking for the specific
  positive outcome the goal actually requires.
- Stopping at the first fix on a multi-bug chain because the symptom
  you started with is gone, without confirming the original ask is
  now actually satisfied end to end.
- Skipping the project's full verification suite after a
  multi-file fix because "I already tested it manually on device" —
  manual verification and automated regression coverage check
  different things; do both before calling it done.
