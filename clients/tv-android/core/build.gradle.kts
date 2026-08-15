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
    implementation(libs.kotlinx.coroutines.core)
    implementation(libs.kotlinx.serialization.json)

    testImplementation(libs.junit.jupiter)
    testImplementation(libs.okhttp.mockwebserver)
    testImplementation(libs.kotlinx.coroutines.test)
}

tasks.test {
    useJUnitPlatform()
}
