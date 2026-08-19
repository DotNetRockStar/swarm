---
name: swarm-real-device-debugging
description: Use when the Fire TV client crashes, misbehaves, has unreachable UI, silently fails to connect/play, or won't launch on a real device but looks fine in compile/unit-test verification — methodology for real-hardware Android debugging (including driving the app's D-pad-only UI via adb), plus five confirmed root causes (a ClassNotFoundException namespace mismatch, an off-screen/unreachable-by-D-pad layout bug, a missing-initial-focus bug on state-swap navigation, an API-33-only method silently NoSuchMethodError-ing on older real hardware, and an IPv6/IPv4 loopback-binding mismatch) and the dead ends that looked plausible first.
---

# Debugging real-Fire-TV-only failures

Real hardware finds bugs nothing else in this repo can: no emulator is
set up, and every automated test either runs in a JVM (`:core:test`) or
against real *Rust* subprocesses (`:core:interopTest`) — neither touches
Android's actual classloader, PackageManager, or a real OEM ART build.
The first real-device run this project ever did (`deploy_tv.sh` against
a real Amazon Fire TV 4-Series, Fire OS 7 / API 28) immediately crashed
on launch, and chasing it down took a long detour before finding the
real, tiny cause. Read this before spending time on a similar crash.

## The actual bug, and check this FIRST

`ClassNotFoundException` on a manifest-registered `Application` or
`Activity` class, where the class is genuinely present in the APK, is
most likely a **namespace/package mismatch**, not a dex-sharding problem
— check this before anything multidex-related:

- AGP's `namespace` (`app/build.gradle.kts`) is `app.swarm.tv`.
- This module's actual Kotlin package is `app.swarm.tv.app` (one segment
  deeper — see any file under `app/src/main/kotlin/app/swarm/tv/app/`).
- A manifest entry like `android:name=".MainActivity"` resolves the
  leading `.` against `namespace`, giving `app.swarm.tv.MainActivity` —
  a class that has never existed. The real class is
  `app.swarm.tv.app.MainActivity`.
- Same for `adb shell am start -n app.swarm.tv/.MainActivity` — the `-n`
  flag resolves a leading `.` the identical way, so driving it by hand
  reproduces the exact same wrong name.

**Fix**: use the fully-qualified class name, not a relative one, anywhere
an Activity/Application/Service is referenced by name — in
`AndroidManifest.xml` and in any `adb shell am start -n`/`-a` invocation
(`deploy_tv.sh`'s `ACTIVITY` variable does this correctly; copy its
pattern, don't reintroduce a relative name). This one fix resolved a
crash that looked, for a long time, like a multidex/dex-placement issue.

## Dead ends that looked plausible — don't re-walk these blind

All of the below were tried, in this order, chasing what turned out to
be the wrong theory (dex-sharding) for the crash above. Each step *did*
teach something real about AGP, kept here so it doesn't need
rediscovering — but none of it was the actual fix, and all of it was
reverted:

1. **`multiDexEnabled = true` alone** — genuinely necessary (16 dex files,
   >65536 methods), but doesn't control *placement* of any specific class.
2. **Custom `Application` + `MultiDexApplication`/`MultiDex.install()`** —
   confirmed via source behavior: this no-ops above API 20
   (`if (VERSION.SDK_INT >= 21) return`), on the (normally correct)
   assumption that native ART multidex just works on any such device.
   Looked like it fixed things because deleting the (empty, unused)
   custom `Application` class removed the mis-resolved manifest
   reference entirely — not because anything about multidex changed.
3. **`multiDexKeepFile`/`multiDexKeepProguard`** — silently no-ops under
   *native* multidex (minSdk ≥ 21); only has any effect once legacy
   dexing is actually active.
4. **Forcing legacy multidex via a lower `minSdk`** — `minSdk` cannot be
   overridden per `buildType` in this AGP DSL (confirmed: `Unresolved
   reference: minSdk` inside `buildTypes { debug { ... } }`), only via
   `defaultConfig`/`productFlavor`. Introduced a whole `device`/`store`
   product-flavor split with `minSdk = 20` for `device` to make legacy
   dexing real for local testing, which in turn required a large
   `tools:overrideLibrary` manifest allowlist (every AndroidX/Compose/
   Media3/Coil library declares its own `minSdkVersion ≥ 21` floor, and
   the manifest merger refuses to build below that without an explicit
   override per library). This **did** get `MainActivity` verified into
   the true primary `classes.dex` (checked with `dexdump`, not a
   string grep — see below) — and the app *still* crashed identically,
   which is what finally pointed at something other than dex placement.
5. **Stale per-package device state** — tested by bumping `versionCode`
   and separately by installing under a brand-new never-before-seen
   `applicationIdSuffix`. Identical crash under a package this device had
   zero history with, which ruled out any install/dexopt cache theory
   entirely and was the last thing checked before rereading the actual
   exception class name character-by-character against the real Kotlin
   package declaration.

All of the flavor/legacy-multidex/override-list machinery was reverted
once the real fix was found — it added real complexity for zero benefit
once the manifest name was correct. If a future crash genuinely does turn
out to be about dex placement (verify with `dexdump`, don't assume), this
is the path that worked; don't reach for it before ruling out a name
mismatch first, given how long the false trail was here.

## Bug #2: off-screen content is unreachable by D-pad (no scroll container)

Found *after* the ClassNotFoundException fix above, while actually
driving the passcode screen end to end: `PasscodeEntryScreen.kt`'s outer
`Column` used `Modifier.fillMaxSize()` with `verticalArrangement =
Arrangement.Center` and no scroll modifier. Its full content (title, two
text fields, 8 digit slots, and all 4 number-pad rows) is taller than a
real 1080p Fire TV viewport. Symptom: `LEFT`/`RIGHT` navigate the number
pad fine, but `DOWN` past row 2 (`4 5 6`) never moves focus at all —
rows 3–4 (`7 8 9` / `0 ⌫`) render below the visible screen and Compose's
focus-search can't reach them. This isn't cosmetic: passcodes are random
8-digit codes, so this silently blocked entry of virtually every real
code, on **every** Fire TV at this resolution, not just one unit.

**Fix**: wrap the outer `Column` in `Modifier.verticalScroll(rememberScrollState())`.
Compose auto-scrolls a scrollable ancestor to keep the focused item in
view as focus moves, so D-pad-only navigation then reaches every row with
no other change needed. Confirmed on two different real Fire TVs (a
4-Series 65" and a second, different model) after the fix: full 8-digit
entry via D-pad, "Join swarm" enabled and reachable, real STUN
registration succeeded, confirmed both on-device and via
`GET /api/v1/me/devices` server-side.

**General lesson**: any full-screen Compose layout in this app that
isn't obviously short (a single button, a short form) needs either a
scroll container or a real on-device visual check — `Arrangement.Center`
alone silently clips overflow instead of erroring, and nothing in
`:core:test`/`:core:interopTest` can catch this since neither renders a
real Compose UI on a real screen size.

## Bug #3: state-swap navigation never requests initial D-pad focus

Found while testing the real end-to-end flow (real user media, real
server, real Fire TV): the "Browse library" button on
`SwarmDashboardScreen` never responded to any D-pad input — CENTER,
TAB, nothing. Ruled out first, in order: adb input delivery (confirmed
via `Input: injectKeyEvent` and Amazon's own
`OZ-DCS:KeyPressReceiver` logs showing the OS genuinely received the
key), wrong-element-focus (tested via TAB, still nothing), and a
button-color-based focus assumption (the button's `containerColor` is
static regardless of focus state, so it visually looks focused-ish
even when nothing is). Root cause, found by reading source: this
screen (and `CatalogScreen`, which has the identical problem) is
reached via a `when`-based `UiState` swap in `SwarmApp`, not a real
Navigation component — nothing ever calls `requestFocus()` on the new
screen's content, so D-pad focus stays wherever it was (nowhere, on a
freshly-composed screen) and every key press is silently a no-op.

**Fix**: `val requester = remember { FocusRequester() }` +
`LaunchedEffect(key) { requester.requestFocus() }`, with
`Modifier.focusRequester(requester)` on whichever element should
receive focus first — the primary action button on
`SwarmDashboardScreen`, the first catalog card on `CatalogScreen`
(computed as index 0 of the first non-empty kind row, passed down
through `CatalogRow`/`CatalogCard`). Confirmed via
`SwarmViewModel`'s existing click-handler logging: zero invocations
before the fix, `browseCatalog() called` immediately after pressing
CENTER post-fix.

**General lesson**: any screen in this app reached via a `UiState`
`when`-swap (check `SwarmApp` in `MainActivity.kt` for the full list)
needs this same fix unless it's been explicitly verified to receive
focus some other way. A visually-focused-looking button proves
nothing — check the actual `colors()` call for a `focusedContainerColor`
override before trusting a screenshot's appearance.

## Bug #4: an API-33-only stdlib call throws NoSuchMethodError on this hardware, silently

Found immediately after fixing Bug #3: "Browse library" now
*responded*, but the catalog always came back `entries=0
unreachable=1` even for a server proven reachable seconds earlier by a
successful raw QUIC connection (`swarm_media::serve: peer connected`
in the server log). `CatalogSession.connectionFor()`/`fetchManifest()`
both swallow their connection attempt in `runCatching { ... }.getOrNull()`
with no logging — by design, since one bad peer shouldn't block
browsing the rest of the swarm, but that same design makes a real bug
indistinguishable from an ordinary timeout. Added `.onFailure {
it.printStackTrace() }` to both (kept permanently — see the code
comment) and the real exception surfaced immediately:

```
java.lang.NoSuchMethodError: No virtual method toString(Ljava/nio/charset/Charset;)Ljava/lang/String; in class Ljava/io/ByteArrayOutputStream;
	at app.swarm.tv.core.transport.PeerQuicClient.readHeaderLine(PeerQuicClient.kt:175)
```

`ByteArrayOutputStream.toString(Charset)` was added in API 33; this
device runs an older Fire OS/Android version, so the method genuinely
doesn't exist in `core-oj.jar` at runtime, even though it compiles
fine (nothing in this project's toolchain runs a real API-level lint —
`:app:lintDebug` is broken/ignored here, see `swarm-local-testing`).
**Fix**: use the `toString(String)` overload instead
(`buffer.toString(Charsets.UTF_8.name())`) — available since API 1,
and already the correct pattern used one file over in
`PeerLoopbackProxy.kt`, which is what made this an isolated mistake
rather than a systemic one (confirmed via `grep -rn
"\.toString(Charsets\.\|\.toString(Charset\b"` across the whole
client — only the one bad call existed).

**General lesson**: a `NoSuchMethodError` (not `ClassNotFoundException`)
on a method from a common JDK class is the signature of an API-level
gap — the class exists, the specific overload doesn't, on *this*
device's real API level. `minSdk` in the Gradle config doesn't stop
you from writing the call; it only documents that you're not supposed
to. Nothing short of a real device below the target API (or a working
lint pass) catches it before runtime.

## Bug #5: loopback proxy binds IPv6, hardcoded URL is IPv4 — different sockets

Found immediately after fixing Bug #4: the catalog now loaded for
real, but selecting a title to play failed with ExoPlayer logging
`ConnectException: Failed to connect to /127.0.0.1:<port>` even though
`PeerLoopbackProxy`'s accept loop was demonstrably alive (it had
already served the manifest fetch that populated the catalog, over
the exact same proxy instance). Root cause:
`PeerLoopbackProxy.start()` bound its `ServerSocket` via
`InetAddress.getLoopbackAddress()`, which resolved to the **IPv6**
loopback (`::1`) on this hardware's libcore, while `urlFor()` hands
callers a hardcoded literal `"http://127.0.0.1:$port/..."` — two
different addresses, so nothing was ever listening on the one the URL
actually pointed at, despite the proxy being genuinely up and healthy
on `::1`.

**Fix**: bind the literal address instead of the ambiguous
platform-dependent helper — `InetAddress.getByName("127.0.0.1")` —
matching the hardcoded URL exactly. Confirmed: real video played,
with `MediaCodecLogger` showing live hardware-decoded `avc`/`mp4a`
bitrates and the frame visibly advancing across repeated screenshots.

**General lesson**: `InetAddress.getLoopbackAddress()` is not
guaranteed to mean "the IPv4 loopback" on every JVM/libcore
implementation — if code elsewhere hardcodes `127.0.0.1` (as a URL, a
cert SAN, an allowlist entry, etc.), bind with the same literal rather
than a helper that might resolve to the IPv6 form on some real device.
This class of bug produces a plain `ConnectException` (connection
refused), not a more specific IPv6-related exception, so it looks
identical to "nothing is listening" — check what's actually bound
(the accept loop being alive is not proof of *which* address it's
alive on) before assuming the server side crashed or never started.

## Driving the app's UI via adb for real-device testing

No touch/tap works on this app's custom TV widgets (`adb shell input tap
x y` on the number-pad buttons and digit slots produces zero visible
effect — this app is D-pad/focus-driven only, consistent with the "no
touchscreen required" Fire TV design goal). Real technique, developed the
hard way:

- **`adb shell input text "..."`** works for the two real `OutlinedTextField`s
  (STUN URL, device name) — it injects into whichever field currently has
  focus, no need to open the on-screen keyboard first.
- **`adb shell input keyevent KEYCODE_TAB`** moves focus forward between
  the two text fields reliably. `KEYCODE_DPAD_DOWN` from a focused text
  field instead opens the on-screen keyboard for that field (a real
  Fire OS behavior, not a bug) — use TAB to move between fields, not DOWN.
- **The number pad has no touch/text-input path at all** — `DigitSlot`
  and the pad buttons are plain `Box`/`Button` composables with no
  backing `TextField`, so `input text` has nothing to attach to. The only
  way in is D-pad: `KEYCODE_DPAD_UP/DOWN/LEFT/RIGHT` + `KEYCODE_DPAD_CENTER`
  to press the focused button. Read `NumberPad`'s `padRows` in
  `PasscodeEntryScreen.kt` to get the exact grid layout
  (`1 2 3 / 4 5 6 / 7 8 9 / _ 0 ⌫`) and compute a navigation path by hand
  — don't guess it from a screenshot alone (see the glyph-rendering note
  below).
- **Batching more than ~2 keyevents in one `adb shell` round trip risks
  silently dropping one** — confirmed twice: a 4-key batch
  (`DOWN,DOWN,RIGHT,CENTER`, 0.4–1.5s apart) landed on the wrong button at
  least once each time, entering a wrong digit without any error. A
  single key + `sleep 1.5` + screenshot-verify, every time, was 100%
  reliable across dozens of presses. Slower, but the only way that didn't
  need backtracking. This got much less finicky (2–3 keys per call became
  reliable) once the Bug #2 scroll fix landed — worth retrying a slightly
  larger batch size after that fix if speed matters, but always verify
  the *result*, not just trust the batch executed.
- **A stray key can navigate clean out of the app — and `KEYCODE_BACK`
  from this app's root screen is the single riskiest key to send.**
  `PasscodeEntryScreen` (and every other top-level `UiState` screen) has
  no back-stack of its own, so `BACK` from there falls through to
  whatever the OS considers "underneath" — which is real user state on a
  real device, not a safe no-op. Confirmed twice, at increasing severity:
  once landed on the Amazon Appstore (`com.amazon.venezia`) browsing
  page; a second time landed **on a live "Get MHz Choice" Prime Video
  channel subscription checkout screen**, mid-flow, with a real saved
  card ("Visa ending in 0053") pre-selected and a one-press "Start MHz
  Choice free trial" button — one more misdirected `KEYCODE_DPAD_CENTER`
  there would have started a real paid subscription trial on the
  account's real payment method. Confirmed via `adb shell dumpsys
  activity activities | grep mResumedActivity` both times, same as
  before. **Never send `BACK` speculatively while driving this app on
  someone's real device with a real Amazon account behind it** — only
  send it when the current screen is positively confirmed (via a fresh
  screenshot) to be a screen *inside* this app that's known to have its
  own back target. If `BACK` (or anything else) lands somewhere
  unexpected, stop issuing keys immediately, screenshot to see what's
  actually on screen, and recover with `adb shell input keyevent
  KEYCODE_HOME` then relaunch
  (`am start -n app.swarm.tv/app.swarm.tv.app.MainActivity`) — never
  press `CENTER`/`DPAD_CENTER`/`ENTER` to try to "back out" of an unknown
  screen, since that's exactly the press that would confirm a purchase.
- **`adb exec-out screencap -p` can prepend stray bytes on this hardware**
  — one capture had a literal log line (`Init wrapper sys mutex
  successful. Pid:2160`) stuck in front of the real PNG data, making it
  fail to decode. Always find the real PNG magic bytes and slice from
  there rather than trusting the raw stream:
  ```python
  data = open(path, 'rb').read()
  idx = data.find(b'\x89PNG\r\n\x1a\n')
  open(fixed_path, 'wb').write(data[idx:] if idx > 0 else data)
  ```
- **Glyph rendering can look broken when it's actually just off-screen/
  unfocused.** Before the Bug #2 fix, unfocused number-pad buttons in the
  clipped region rendered as tiny illegible marks that looked like a font
  problem — they weren't; the real digits (`4 5 6` etc.) were there all
  along, just barely visible pre-fix. Don't chase a font/glyph theory
  before checking whether it's actually a layout/scroll issue.
- **A device can reboot mid-session** (e.g. if the user power-cycles it
  after seeing something odd like the Appstore launching unexpectedly).
  `adb`'s TCP connection silently goes stale; `adb disconnect <ip>:5555 &&
  adb connect <ip>:5555` before the next command re-establishes it — a
  `get-state` check first confirms it's really back.

## Verification methodology: don't trust a short sleep window

A `sleep 3` + one-time crash check reported the app as working right
after the (wrong, multidex-based) fix — it wasn't; on this specific
hardware the same crash took as long as ~5 seconds to actually surface in
logcat after process start, well past a 3-4s check. A single short sleep
produces false positives on this hardware. Always poll:

```bash
for _ in $(seq 1 8); do
    sleep 2
    CRASH="$(adb -s "$SERIAL" logcat -d | grep "FATAL EXCEPTION" || true)"
    PID="$(adb -s "$SERIAL" shell pidof "$PACKAGE" || true)"
    [ -n "$CRASH" ] && break
done
```
`deploy_tv.sh` already does this (16s total, 8×2s). Don't shorten it back
down without re-confirming the timing on real hardware first.

## Useful commands for this class of bug

**Pull the actual installed APK (don't trust your local build output
matches what's really on the device):**
```bash
PKGPATH=$(adb -s "$SERIAL" shell pm path app.swarm.tv | tr -d '\r' | sed 's/package://')
adb -s "$SERIAL" pull "$PKGPATH" /tmp/installed.apk
```

**Check which dex file a class is really DEFINED in (not just
mentioned/referenced — a plain `grep`/`strings` on a `.dex` file matches
both, and that distinction matters):**
```bash
unzip -o installed.apk classes.dex classes2.dex   # etc, or all classes*.dex
DEXDUMP=$(find ~/Library/Android/sdk/build-tools -name dexdump | head -1)
"$DEXDUMP" classes.dex | grep -A1 "Class descriptor.*YourClassName;'"
```
A hit under `Class descriptor` is a real definition; a bare string match
elsewhere in the file (e.g. via `strings`/`grep -a`) can just be another
class *referencing* the name (a lambda, a lifecycle callback, an `Intent`
extra) and proves nothing about where the actual class lives.

**Find every dependency's own declared `minSdkVersion` floor** (needed
once, to build the `tools:overrideLibrary` list during the abandoned
legacy-multidex experiment — kept here in case a similar cross-dependency
survey is ever needed again for something else):
```bash
cd ~/.gradle/caches/*/transforms
for f in */transformed/*/AndroidManifest.xml; do
  pkg=$(grep -o 'package="[^"]*"' "$f" | head -1 | sed 's/package="//;s/"//')
  minsdk=$(grep -o 'android:minSdkVersion="[0-9]*"' "$f" | head -1 | grep -o '[0-9]*')
  [ -n "$pkg" ] && [ -n "$minsdk" ] && [ "$minsdk" -ge 21 ] 2>/dev/null && echo "$pkg (minSdk=$minsdk)"
done | sort -u
```

**Watch a crash happen live, with no rebuild** — `tv_logs.sh` (repo
root); see `swarm-local-testing` skill.

## General lesson for next time

When a real-device crash *looks like* it matches a well-known Android
gotcha (multidex is an easy one to reach for — it's real, documented, and
this project genuinely does ship 16 dex files), verify the mundane
explanation first: read the exact class name in the exception character
by character against the real source file's `package` declaration. It's
a five-second check that would have skipped the entire detour above.
