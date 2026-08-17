---
name: swarm-real-device-debugging
description: Use when the Fire TV client crashes, misbehaves, or won't launch on a real device but looks fine in compile/unit-test verification — methodology for real-hardware Android debugging, plus a specific ClassNotFoundException root cause and the dead ends that looked plausible first.
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
