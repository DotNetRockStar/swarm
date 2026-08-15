package app.swarm.tv.core.transport

import app.swarm.tv.core.rest.DeviceType
import app.swarm.tv.core.rest.SwarmDevice
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Test

class PeerAddressTest {
    @Test
    fun `parses an ipv4 host and port`() {
        assertEquals(PeerAddress("192.168.1.50", 8543), PeerAddress.parse("192.168.1.50:8543"))
    }

    @Test
    fun `parses a bracketed ipv6 host and port`() {
        assertEquals(PeerAddress("::1", 8543), PeerAddress.parse("[::1]:8543"))
    }

    @Test
    fun `rejects malformed values`() {
        val malformed = listOf(
            "", "no-colon", "host:", ":1234", "host:notaport",
            "host:0", "host:70000", "[::1]", "[::1]:", "[]:1234",
        )
        for (value in malformed) {
            assertNull(PeerAddress.parse(value), "expected null for '$value'")
        }
    }

    @Test
    fun `connectToServer returns null when the device has not self-reported an address yet`() {
        val identity = TestIdentity.generate("caller")
        val device = SwarmDevice(
            deviceId = "dev-1",
            name = "Server",
            deviceType = DeviceType.SERVER,
            certFingerprint = "ab".repeat(32),
            online = true,
            metadata = emptyMap(),
        )
        assertNull(connectToServer(device, identity.certificate, identity.privateKey))
    }

    @Test
    fun `connectToServer returns null when peer_addr is malformed`() {
        val identity = TestIdentity.generate("caller")
        val device = SwarmDevice(
            deviceId = "dev-1",
            name = "Server",
            deviceType = DeviceType.SERVER,
            certFingerprint = "ab".repeat(32),
            online = true,
            metadata = mapOf("peer_addr" to "not-an-address"),
        )
        assertNull(connectToServer(device, identity.certificate, identity.privateKey))
    }
}
