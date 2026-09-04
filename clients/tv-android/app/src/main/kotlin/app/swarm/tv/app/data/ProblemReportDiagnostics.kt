package app.swarm.tv.app.data

import android.app.ActivityManager
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.BatteryManager
import android.os.Build
import android.os.Environment
import android.os.Process
import android.os.StatFs
import android.os.SystemClock
import app.swarm.tv.BuildConfig
import app.swarm.tv.core.catalog.MergedEntry
import app.swarm.tv.core.catalog.displayTitle
import app.swarm.tv.core.rest.SwarmDevice
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.TimeZone

private const val RECENT_LOG_LINE_COUNT = 200
internal const val MAX_RECENT_LOG_CHARS = 20_000

fun interface ProblemReportDiagnostics {
    /** Called on an IO dispatcher. Implementations must return useful fallback text instead of throwing. */
    fun collect(): String
}

/**
 * Captures details that are only available from Android at runtime. Logcat is
 * deliberately read from the app process rather than requiring READ_LOGS;
 * modern Android/Fire OS permits an app to see its own UID's output. The
 * bounded tail leaves ample room under the peer protocol's 64 KiB request
 * header limit after JSON escaping.
 */
class AndroidProblemReportDiagnostics(context: Context) : ProblemReportDiagnostics {
    private val appContext = context.applicationContext

    override fun collect(): String = buildString {
        appendLine("Client runtime")
        field("captured_at", formatTimestamp(System.currentTimeMillis()))
        field("app", "${BuildConfig.APPLICATION_ID} ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE}) ${BuildConfig.BUILD_TYPE}")
        field("device_uptime_secs", SystemClock.elapsedRealtime() / 1_000)
        field("process_uptime_secs", (SystemClock.elapsedRealtime() - Process.getStartElapsedRealtime()) / 1_000)
        field("device", listOf(Build.MANUFACTURER, Build.MODEL, Build.DEVICE, Build.PRODUCT).joinToString(" / "))
        field("os", "Android ${Build.VERSION.RELEASE} (SDK ${Build.VERSION.SDK_INT}, build ${Build.DISPLAY})")
        field("abis", Build.SUPPORTED_ABIS.joinToString())
        field("locale", appContext.resources.configuration.locales[0].toLanguageTag())
        field("timezone", TimeZone.getDefault().id)

        runCatching {
            val metrics = appContext.resources.displayMetrics
            field("display", "${metrics.widthPixels}x${metrics.heightPixels} px, ${metrics.densityDpi} dpi")
        }.onFailure { field("display", "unavailable (${it.javaClass.simpleName})") }

        runCatching {
            val manager = appContext.getSystemService(Context.ACTIVITY_SERVICE) as ActivityManager
            val system = ActivityManager.MemoryInfo().also(manager::getMemoryInfo)
            val runtime = Runtime.getRuntime()
            field(
                "memory",
                "system_available=${system.availMem}; system_total=${system.totalMem}; low=${system.lowMemory}; " +
                    "app_used=${runtime.totalMemory() - runtime.freeMemory()}; app_max=${runtime.maxMemory()}",
            )
        }.onFailure { field("memory", "unavailable (${it.javaClass.simpleName})") }

        runCatching {
            val stats = StatFs(Environment.getDataDirectory().absolutePath)
            field("storage", "available=${stats.availableBytes}; total=${stats.totalBytes}")
        }.onFailure { field("storage", "unavailable (${it.javaClass.simpleName})") }

        runCatching {
            val manager = appContext.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
            val capabilities = manager.getNetworkCapabilities(manager.activeNetwork)
            val transports = buildList {
                if (capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) == true) add("ethernet")
                if (capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) == true) add("wifi")
                if (capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) == true) add("cellular")
                if (capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_VPN) == true) add("vpn")
            }
            field(
                "network",
                "transport=${transports.ifEmpty { listOf("none") }.joinToString("+")}; " +
                    "validated=${capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED) == true}; " +
                    "metered=${manager.isActiveNetworkMetered}; " +
                    "down_kbps=${capabilities?.linkDownstreamBandwidthKbps ?: 0}; " +
                    "up_kbps=${capabilities?.linkUpstreamBandwidthKbps ?: 0}",
            )
        }.onFailure { field("network", "unavailable (${it.javaClass.simpleName})") }

        runCatching {
            val battery = appContext.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
            val level = battery?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1) ?: -1
            val scale = battery?.getIntExtra(BatteryManager.EXTRA_SCALE, -1) ?: -1
            val status = battery?.getIntExtra(BatteryManager.EXTRA_STATUS, -1) ?: -1
            val percent = if (level >= 0 && scale > 0) level * 100 / scale else -1
            val charging = status == BatteryManager.BATTERY_STATUS_CHARGING || status == BatteryManager.BATTERY_STATUS_FULL
            field("battery", "percent=$percent; charging=$charging")
        }.onFailure { field("battery", "unavailable (${it.javaClass.simpleName})") }

        appendLine()
        appendLine("Recent application logs (newest ${RECENT_LOG_LINE_COUNT} lines requested)")
        append(readRecentLogs())
    }.trimEnd()

    private fun readRecentLogs(): String {
        val commands = listOf(
            listOf("logcat", "-d", "-v", "threadtime", "--pid=${Process.myPid()}", "-t", RECENT_LOG_LINE_COUNT.toString()),
            listOf("logcat", "-d", "-v", "threadtime", "-t", RECENT_LOG_LINE_COUNT.toString()),
        )
        var failure = "logcat unavailable"
        for (command in commands) {
            val result = runCatching { runLogcat(command) }
                .onFailure { failure = "logcat unavailable (${it.javaClass.simpleName})" }
                .getOrNull() ?: continue
            if (result.exitCode == 0) {
                return sanitizeAndBoundRecentLogs(result.output).ifBlank { "<no readable application log entries>" }
            }
            failure = "logcat exited ${result.exitCode}"
        }
        return "<$failure>"
    }

    private fun runLogcat(command: List<String>): LogcatResult {
        val process = ProcessBuilder(command).redirectErrorStream(true).start()
        return try {
            val output = process.inputStream.bufferedReader().use { it.readText() }
            LogcatResult(process.waitFor(), output)
        } finally {
            process.destroy()
        }
    }

    private data class LogcatResult(val exitCode: Int, val output: String)
}

/**
 * Builds the diagnostic context shared by user-initiated problem reports and
 * automatic client errors. Automatic errors are not always tied to an asset
 * (catalog refresh failures, for example), so [entry] is optional while the
 * application, reporting-server, and runtime sections are always present.
 */
internal fun buildClientErrorContext(
    entry: MergedEntry?,
    device: SwarmDevice,
    screen: String,
    connectionMode: String,
    clientDeviceId: String?,
    clientMachineId: String,
    clientCertFingerprint: String,
    swarmId: String?,
    catalogEntryCount: Int,
    catalogServerCount: Int,
    unreachableServerIds: List<String>,
    playbackError: String?,
    pendingReportCount: Int,
    kidModeEnabled: Boolean,
    shuffleMode: String,
    minimizedTitle: String?,
    previewEntryKey: String?,
    errorDetails: String?,
    runtimeDiagnostics: String,
): String = buildString {
    appendLine("Application state")
    field("screen", screen)
    field("connection_mode", connectionMode)
    field("client_device_id", clientDeviceId ?: "unknown")
    field("client_machine_id", clientMachineId)
    field("client_cert_fingerprint", clientCertFingerprint)
    field("swarm_id", swarmId ?: "none")
    field("catalog_entries", catalogEntryCount)
    field("catalog_servers", catalogServerCount)
    field("unreachable_servers", unreachableServerIds.ifEmpty { listOf("none") }.joinToString())
    field("playback_error", playbackError ?: "none")
    field("pending_problem_reports", pendingReportCount)
    field("family_mode", kidModeEnabled)
    field("shuffle", shuffleMode)
    field("minimized_playback", minimizedTitle ?: "none")
    field("browse_preview_entry", previewEntryKey ?: "none")

    if (entry != null) {
        appendLine()
        appendLine("Asset")
        field("entry_key", entry.entry.entryKey)
        field("fingerprint", entry.entry.fingerprint)
        field("kind", entry.entry.kind.name.lowercase())
        field("display_title", entry.entry.displayTitle())
        field("library_title", entry.entry.title)
        field("sources", entry.sources.joinToString())
        field("size_bytes", entry.entry.size)
        field("duration_secs", entry.entry.durationSecs ?: "unknown")
        field("show", entry.entry.showTitle ?: "none")
        field("season_episode", "${entry.entry.season ?: "none"}/${entry.entry.episode ?: "none"}")
        field("artist_album_track", "${entry.entry.artist ?: "none"} / ${entry.entry.album ?: "none"} / ${entry.entry.trackNumber ?: "none"}")
        field("year", entry.entry.year ?: "unknown")
        field("rating", entry.entry.rating ?: "unknown")
        field("genres", entry.entry.genres.ifEmpty { listOf("none") }.joinToString())
        field(
            "video",
            entry.entry.video?.let { "codec=${it.codec}; ${it.width}x${it.height}; level=${it.level ?: "unknown"}; bitrate=${it.bitrate ?: "unknown"}" }
                ?: "none",
        )
        field(
            "audio",
            entry.entry.audio?.let { "codec=${it.codec}; channels=${it.channels}; bitrate=${it.bitrate ?: "unknown"}" } ?: "none",
        )
        field("artwork_etag", entry.entry.artworkEtag ?: "none")
    }

    appendLine()
    appendLine("Reporting server")
    field("device_id", device.deviceId)
    field("name", device.name)
    field("type", device.deviceType.name.lowercase())
    field("cert_fingerprint", device.certFingerprint)
    field("online", device.online)
    field("last_seen", device.lastSeenAt ?: "unknown")
    device.metadata.toSortedMap().forEach { (key, value) -> field("metadata.$key", value) }

    if (!errorDetails.isNullOrBlank()) {
        appendLine()
        appendLine("Error details")
        appendLine(errorDetails.trim())
    }

    appendLine()
    append(runtimeDiagnostics)
}.trimEnd()

internal fun sanitizeAndBoundRecentLogs(raw: String, maxChars: Int = MAX_RECENT_LOG_CHARS): String {
    require(maxChars > 0)
    val redacted = raw
        .replace(Regex("(?i)\\bBearer\\s+[A-Za-z0-9._~+/=-]+"), "Bearer <redacted>")
        .replace(
            Regex("(?i)\\b(authorization|access[_ -]?token|poll[_ -]?token|refresh[_ -]?token|password)(\\s*[:=]\\s*|\\s+)([^\\s,;]+)"),
            "$1$2<redacted>",
        )
        .trim()
    if (redacted.length <= maxChars) return redacted
    val tail = redacted.takeLast(maxChars)
    val firstCompleteLine = tail.indexOf('\n').takeIf { it >= 0 }?.let { tail.substring(it + 1) } ?: tail
    return "<older log output truncated>\n$firstCompleteLine"
}

private fun StringBuilder.field(name: String, value: Any) {
    append(name)
    append('=')
    appendLine(value.toString().replace('\r', ' ').replace('\n', ' '))
}

private fun formatTimestamp(timestampMs: Long): String =
    SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss.SSS'Z'", Locale.US).apply {
        timeZone = TimeZone.getTimeZone("UTC")
    }.format(Date(timestampMs))
