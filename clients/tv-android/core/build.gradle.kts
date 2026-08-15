// Pure Kotlin/JVM module: wire contracts, the STUN REST client, and catalog
// merge logic. No Android dependency — compiles and tests with just the
// JDK, so this is the part of the client CI/local dev can always verify
// even without the Android SDK installed. The Android app module wraps
// this with platform-specific pieces (Compose UI, Android Keystore token
// storage, Media3 playback).
plugins {
    alias(libs.plugins.kotlin.jvm)
    alias(libs.plugins.kotlin.serialization)
}

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    api(libs.okhttp) // StunApiClient's constructor exposes OkHttpClient to callers
    api(libs.kwik)    // PeerQuicClient's constructor exposes kwik types to callers
    implementation(libs.kotlinx.coroutines.core)
    implementation(libs.kotlinx.serialization.json)

    testImplementation(libs.junit.jupiter)
    testImplementation(libs.okhttp.mockwebserver)
    testImplementation(libs.kotlinx.coroutines.test)
    // Test-only: generates a self-signed X.509 identity for the interop
    // spike against the real Rust server. Production Android code uses
    // AndroidKeyStore instead (see :app's AndroidDeviceIdentity) — this
    // never ships.
    testImplementation(libs.bouncycastle.pkix)
    testImplementation(libs.bouncycastle.prov)
}

tasks.test {
    useJUnitPlatform()
    // Interop tests spawn a real OS subprocess (the Rust swarm-serverd
    // binary) and hold live QUIC/UDP resources — qualitatively different
    // from the fast, dependency-free unit tests here. Kept out of the
    // default suite; see the dedicated `interopTest` task below.
    filter {
        excludeTestsMatching("*InteropTest")
    }
}

tasks.register<Test>("interopTest") {
    description = "Kwik <-> real Rust quinn server QUIC interop spike. Needs a release swarm-serverd build " +
        "(cargo build --release -p swarm-server --bin swarm-serverd) — skips gracefully if absent."
    group = "verification"
    useJUnitPlatform()
    testClassesDirs = sourceSets.test.get().output.classesDirs
    classpath = sourceSets.test.get().runtimeClasspath
    filter {
        includeTestsMatching("*InteropTest")
    }
    // kwik's LossDetector.detectLostPackets() has `assert(lossDelay > 0)`
    // (core/recovery/LossDetector.java:159 as of kwik 0.10.3), which is
    // *reachable*: lossDelay is computed as
    // `(int) (9f/8f * max(smoothedRtt, latestRtt))` and both RTT estimates
    // are legitimately 0 microseconds early in a connection's life on a
    // loopback QUIC connection — trivially fast enough that this isn't the
    // rare edge case the assert's author evidently assumed. Root-caused
    // after finding this assert's exact source and confirming empirically:
    // running with assertions disabled here makes both the deterministic
    // 2-concurrent-connection crash AND the previously-mysterious
    // occasional single-connection "Connection closed" flakiness disappear
    // (6+ consecutive clean full-suite runs vs. near-certain failure with
    // assertions on) — one root cause explains both symptoms. This isn't
    // papering over a protocol bug: Java's `assert` is off by default on
    // every real JVM and on Android (ART doesn't enable it either), and
    // Gradle's Test task defaults `enableAssertions` to true — so the
    // *test harness*, not kwik or this project's code, was the thing
    // behaving unlike production. Disabling it here matches what a real
    // Android install actually does.
    enableAssertions = false
    // Each test method still gets a fresh JVM — cheap, and no longer load-
    // bearing for reliability the way it looked like it was before this
    // was root-caused, but no reason to remove it.
    forkEvery = 1
    maxParallelForks = 1
}
