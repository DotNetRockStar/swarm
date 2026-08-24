package app.swarm.tv.app.data

import app.swarm.tv.core.catalog.MergedEntry
import app.swarm.tv.core.peer.AudioStreamInfo
import app.swarm.tv.core.peer.CatalogEntry
import app.swarm.tv.core.peer.MediaKind
import app.swarm.tv.core.peer.VideoStreamInfo
import app.swarm.tv.core.rest.DeviceType
import app.swarm.tv.core.rest.SwarmDevice
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class ProblemReportDiagnosticsTest {
    @Test
    fun `recent logs redact credentials and retain the newest bounded output`() {
        val logs = "Authorization: Bearer secret-token\naccess_token=also-secret\nold line\nnewest line"

        val result = sanitizeAndBoundRecentLogs(logs, maxChars = 30)

        assertFalse(result.contains("secret-token"))
        assertFalse(result.contains("also-secret"))
        assertTrue(result.contains("<older log output truncated>"))
        assertTrue(result.endsWith("newest line"))
    }

    @Test
    fun `problem context includes app asset server and runtime details`() {
        val entry = MergedEntry(
            fingerprint = "fingerprint",
            sources = listOf("server-1"),
            entry = CatalogEntry(
                entryKey = "movie-1",
                fingerprint = "fingerprint",
                kind = MediaKind.MOVIE,
                title = "Movie",
                size = 1234,
                durationSecs = 90.0,
                genres = listOf("Drama"),
                video = VideoStreamInfo("h264", 1920, 1080, bitrate = 8_000_000),
                audio = AudioStreamInfo("aac", 6, 384_000),
            ),
        )
        val device = SwarmDevice(
            deviceId = "server-1",
            name = "Living Room Server",
            deviceType = DeviceType.SERVER,
            certFingerprint = "cert",
            online = true,
            metadata = mapOf("connection_route" to "direct", "peer_addr" to "192.0.2.1:8544"),
        )

        val result = buildAssetProblemContext(
            entry = entry,
            device = device,
            screen = "MovieDetail",
            connectionMode = "lan",
            clientDeviceId = "tv-1",
            clientMachineId = "machine-1",
            clientCertFingerprint = "client-cert",
            swarmId = "swarm-1",
            catalogEntryCount = 42,
            catalogServerCount = 2,
            unreachableServerIds = listOf("server-2"),
            playbackError = null,
            pendingReportCount = 1,
            kidModeEnabled = false,
            shuffleEnabled = false,
            minimizedTitle = null,
            previewEntryKey = null,
            runtimeDiagnostics = "Client runtime\nos=Fire OS",
        )

        listOf(
            "screen=MovieDetail",
            "client_device_id=tv-1",
            "catalog_entries=42",
            "entry_key=movie-1",
            "video=codec=h264; 1920x1080",
            "metadata.connection_route=direct",
            "Client runtime",
            "os=Fire OS",
        ).forEach { expected -> assertTrue(result.contains(expected), "missing $expected") }
    }
}
