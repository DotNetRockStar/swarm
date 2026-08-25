package app.swarm.tv.app.ui.screens

import app.swarm.tv.app.data.LanServer
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Test

class ServerStatusTest {
    private val saved = LanServer(
        serviceName = "SWARM Media Server abc",
        name = "Living Room",
        host = "192.168.0.10",
        peerPort = 8543,
        pairingPort = 8544,
        certFingerprint = "ab".repeat(32),
    )

    @Test
    fun `paired server remains visible as offline when discovery is lost`() {
        assertEquals(
            listOf(LanServerRowState(saved, online = false)),
            knownLanServerRows(discovered = emptyList(), paired = listOf(saved)),
        )
    }

    @Test
    fun `discovery marks paired server connected and supplies its current route`() {
        val discovered = saved.copy(host = "192.168.0.22", peerPort = 9553)

        assertEquals(
            listOf(LanServerRowState(discovered, online = true)),
            knownLanServerRows(discovered = listOf(discovered), paired = listOf(saved)),
        )
    }

    @Test
    fun `server status uses user facing connection terms`() {
        assertEquals("connected", connectionStatusLabel(online = true, disconnected = false))
        assertEquals("offline", connectionStatusLabel(online = false, disconnected = false))
        assertEquals("disconnected", connectionStatusLabel(online = true, disconnected = true))
    }
}
