package app.swarm.tv.app.data

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Test

class LanServerRouteTest {
    private val saved = LanServer(
        serviceName = "SWARM Media Server abc",
        name = "SWARM Media Server",
        host = "192.168.0.242",
        peerPort = 8544,
        pairingPort = 8544,
        certFingerprint = "ab".repeat(32),
    )

    @Test
    fun `same certificate adopts newly discovered DHCP address without pairing`() {
        val discovered = saved.copy(host = "192.168.0.235")

        assertEquals(discovered, preferDiscoveredLanServer(saved, listOf(discovered)))
    }

    @Test
    fun `different certificate cannot replace saved trusted server`() {
        val untrusted = saved.copy(
            host = "192.168.0.235",
            certFingerprint = "cd".repeat(32),
        )

        assertEquals(saved, preferDiscoveredLanServer(saved, listOf(untrusted)))
    }

    @Test
    fun `fingerprint comparison accepts normalized certificate text`() {
        val discovered = saved.copy(
            host = "192.168.0.235",
            certFingerprint = saved.certFingerprint.uppercase(),
        )

        assertEquals(discovered, preferDiscoveredLanServer(saved, listOf(discovered)))
    }
}
