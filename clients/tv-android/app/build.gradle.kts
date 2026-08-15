plugins {
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.serialization)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.android.application)
}

android {
    namespace = "app.swarm.tv"
    compileSdk = 35

    defaultConfig {
        applicationId = "app.swarm.tv"
        // Fire OS 6+ (2018+ devices) — see docs/PROTOCOL.md / the project
        // plan for why the floor isn't lower: Fire OS 5 predates the modern
        // Keystore/Compose/Media3 stack this app is built on.
        minSdk = 25
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    packaging {
        resources.excludes.add("META-INF/*.version")
    }

    // `:app:lintDebug` currently crashes outright on this AGP 8.7.3 / Kotlin
    // 2.0.21 combination (IncompatibleClassChangeError deep in lint's own
    // Kotlin-analysis-API/UAST traversal — at least
    // NonNullableMutableLiveDataDetector and RememberInCompositionDetector
    // both hit it on unrelated code, so this is a toolchain version-skew
    // bug, not a real finding; `abortOnError` doesn't help since the
    // exception happens during analysis, before any result is produced).
    // `compileDebugKotlin`/`assembleDebug` both succeed cleanly and are the
    // real verification signal for now — revisit lint on the next AGP/
    // Kotlin bump.

    // Fire TV Appstore submissions are APKs targeting armeabi-v7a / arm64-v8a
    // (see the project plan's Fire TV client section).
    splits {
        abi {
            isEnable = true
            reset()
            include("armeabi-v7a", "arm64-v8a")
            isUniversalApk = false
        }
    }
}

dependencies {
    implementation(project(":core"))

    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.lifecycle.viewmodel.compose)

    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.tooling.preview)
    debugImplementation(libs.compose.ui.tooling)
    // Phone-flavored Material3 alongside TV Material3, used only for the
    // one-time text entry (STUN URL, device name) that TV Material3 has no
    // component for — standard practice for TV apps' occasional free text.
    implementation(libs.compose.material3)

    implementation(libs.tv.material)
    implementation(libs.tv.foundation)

    implementation(libs.media3.exoplayer)
    implementation(libs.media3.common)
    implementation(libs.media3.hls)
    implementation(libs.media3.ui)

    // Room lands with local catalog persistence in a later pass — not
    // needed yet (no DAO/entity code exists), and pulling it in would need
    // the KSP plugin for nothing.
    implementation(libs.security.crypto)

    implementation(libs.kotlinx.coroutines.android)
}
