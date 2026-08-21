/**
 * A human-friendly default for the passcode screen's "Device name" field —
 * resolved from the TV itself rather than a hardcoded "Fire TV" literal,
 * so a household with more than one device doesn't see every registration
 * show up under the same generic name. [SwarmSettingsScreen] already lets
 * this be overridden/renamed after registration; this only picks the
 * starting value.
 */
package app.swarm.tv.app.data

import android.content.Context
import android.os.Build
import android.provider.Settings

fun resolveDeviceName(context: Context): String {
    // Settings.Global.DEVICE_NAME is the Bluetooth/network-visible name a
    // user typically already set for this exact TV (e.g. via the Alexa
    // app — "Living Room Fire TV"), so it's tried first as the most
    // specific, most likely-already-meaningful name. Build.MODEL (e.g.
    // "AFTKA") is a real but cryptic fallback — still better than a bare
    // literal that can't tell two TVs apart.
    val settingsName = runCatching { Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME) }.getOrNull()
    return settingsName?.trim()?.takeIf { it.isNotEmpty() } ?: Build.MODEL?.takeIf { it.isNotBlank() } ?: "Fire TV"
}
