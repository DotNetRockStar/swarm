plugins {
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.serialization)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.android.application)
    alias(libs.plugins.ksp)
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
        // Well above the 65536 method limit (Compose + Media3 + kwik +
        // coroutines + BouncyCastle land 16 dex files), so this is
        // required for the build to link at all.
        multiDexEnabled = true
        val rendezvousUrl = providers.gradleProperty("swarmRendezvousUrl")
            .orElse(providers.environmentVariable("SWARM_RENDEZVOUS_URL"))
            .orElse("")
            .get()
            .replace("\\", "\\\\")
            .replace("\"", "\\\"")
        buildConfigField("String", "SWARM_RENDEZVOUS_URL", "\"$rendezvousUrl\"")
    }

    buildFeatures { buildConfig = true }

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

// Room writes a JSON snapshot of the schema at each version here on every
// build — the diffable history a migration-testing tool checks new
// Migration objects against (see MigrationTest and data/db/Migrations.kt's
// doc comment for the yoyo-style versioned-script convention this backs).
ksp {
    arg("room.schemaLocation", "$projectDir/schemas")
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

    // Artwork over the same peer QUIC connection as media, via the loopback
    // proxy. Coil 2.x, not 3.x: Coil 3's own dependencies need Kotlin
    // 2.2+/compileSdk 36, both ahead of what this project is pinned to —
    // 2.x needs neither and needs no separate network-engine artifact
    // either (bundles OkHttp fetching by default).
    implementation(libs.coil.compose)
    // Coil's GIF frame decoder — the base coil-compose artifact only handles
    // static formats, so an animated GIF (the player's loading indicator)
    // would otherwise render as its first frame and never animate.
    implementation(libs.coil.gif)

    // Relational on-device store for what the app remembers between
    // launches — the saved STUN connection (server URL, device name,
    // device id) and its joined swarms, plus app-level settings (artwork
    // cache TTL). See data/db/ for the schema and the migration
    // convention. Resume/watched state is a *separate* concern and
    // deliberately does NOT use Room — see AndroidWatchStateStore's doc
    // comment for why a plain key/value store fits that data better.
    implementation(libs.room.runtime)
    implementation(libs.room.ktx)
    ksp(libs.room.compiler)

    implementation(libs.security.crypto)

    implementation(libs.kotlinx.coroutines.android)
    // AndroidWatchStateStore talks to kotlinx.serialization directly
    // (encodeToString/decodeFromString) rather than only through :core's
    // SwarmJson value — :core's own dependency on this is `implementation`,
    // not exposed transitively, so :app needs it too.
    implementation(libs.kotlinx.serialization.json)
}
