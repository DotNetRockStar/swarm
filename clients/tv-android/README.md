# SWARM TV client (Fire TV) — Phase 3

Native Kotlin, single-activity Jetpack Compose for TV (`androidx.tv:tv-material`),
Media3/ExoPlayer, Room. minSdk 25 (Fire OS 6+, 2018+ devices). APK with
armeabi-v7a + arm64-v8a for Amazon Appstore submission.

Key pieces:
- Passcode onboarding (8 digit boxes + D-pad number grid; no system keyboard).
- Access token + device key encrypted with an Android Keystore AES/GCM key
  (hand-rolled wrapper; EncryptedSharedPreferences is unreliable on Fire OS).
- REST client generated from `openapi/` (CI keeps it in sync with swarm-core).
- WSS signaling via OkHttp; QUIC via kwik (quiche-JNI fallback if the Phase 3
  throughput spike fails); loopback HTTP proxy feeding ExoPlayer.
- Screens: swarm switcher, merged library (Continue Watching / Movies / Shows /
  Music), detail, player (foreground service), diagnostics (NAT type, punch
  results, per-server RTT).

Appstore compliance: LEANBACK_LAUNCHER intent, `touchscreen required=false`,
full D-pad reachability, no Google-Play-Services dependencies, privacy policy
URL, content rating questionnaire.
