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
    // Each test method gets a fresh JVM (forkEvery = 1) — this cuts down
    // cross-test flakiness substantially but does not eliminate it: even a
    // single test run in complete JVM isolation occasionally hits
    // `IOException: Connection closed` (a real, unresolved finding, not
    // papered over — see PeerQuicClientInteropTest's class doc). Since a
    // fresh JVM rules out same-process resource reuse as the sole cause,
    // this points more toward OS-level timing/socket-reuse sensitivity in
    // kwik's real-clock loss-detection/ACK logic on a loaded dev machine
    // than a same-JVM leak specifically. Real usage holds one long-lived
    // connection, not rapid reconnect cycles, so this doesn't block the
    // interop conclusion, but it's worth deeper investigation before
    // leaning on kwik for a reconnect-heavy path.
    forkEvery = 1
    maxParallelForks = 1
}
