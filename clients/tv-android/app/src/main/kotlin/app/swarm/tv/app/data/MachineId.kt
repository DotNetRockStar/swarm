/**
 * Stable per-install device identity submitted at registration. Uses
 * `Settings.Secure.ANDROID_ID`, which is unique per app-signing-key +
 * user + device and stable across reinstalls of the *same* signed app
 * (resets on factory reset) — a reasonable Android-native equivalent of
 * the Rust side's persisted-random-id approach in
 * `swarm-stun-client::machine_id`, without needing any extra storage.
 */
package app.swarm.tv.app.data

import android.annotation.SuppressLint
import android.content.Context
import android.provider.Settings

@SuppressLint("HardwareIds")
fun androidMachineId(context: Context): String =
    Settings.Secure.getString(context.contentResolver, Settings.Secure.ANDROID_ID) ?: "unknown-device"
