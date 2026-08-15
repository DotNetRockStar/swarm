/**
 * Where to dial a [SwarmDevice] advertised as a server — the STUN roster on
 * its own only says a server exists, never where it is; a server
 * self-reports that separately via `PATCH /devices/{id}/metadata` (see
 * `swarm-p2p::local_addr` on the Rust side), landing in
 * `SwarmDevice.metadata["peer_addr"]` as the wire form of Rust's
 * `SocketAddr::to_string()` — `host:port` for IPv4, `[host]:port` for IPv6.
 */
package app.swarm.tv.core.transport

import app.swarm.tv.core.rest.SwarmDevice
import java.io.IOException
import java.security.PrivateKey
import java.security.cert.X509Certificate
import java.time.Duration

data class PeerAddress(val host: String, val port: Int) {
    companion object {
        fun parse(value: String): PeerAddress? {
            val trimmed = value.trim()
            if (trimmed.isEmpty()) return null
            return if (trimmed.startsWith('[')) parseBracketed(trimmed) else parsePlain(trimmed)
        }

        private fun parseBracketed(value: String): PeerAddress? {
            val close = value.indexOf(']')
            if (close < 0 || close + 1 >= value.length || value[close + 1] != ':') return null
            val host = value.substring(1, close)
            val port = value.substring(close + 2).toIntOrNull() ?: return null
            return if (host.isNotEmpty() && port in 1..65535) PeerAddress(host, port) else null
        }

        private fun parsePlain(value: String): PeerAddress? {
            val colon = value.lastIndexOf(':')
            if (colon <= 0 || colon == value.length - 1) return null
            val port = value.substring(colon + 1).toIntOrNull() ?: return null
            return if (port in 1..65535) PeerAddress(value.substring(0, colon), port) else null
        }
    }
}

/**
 * Connects to [device] using its self-reported `peer_addr`, pinning
 * [SwarmDevice.certFingerprint] from the roster as the trust anchor. Returns
 * null — rather than throwing — when the device has no usable address yet
 * (not self-reported, or malformed), so a caller working through a whole
 * roster can skip one not-yet-ready device without treating it as an error.
 * A real connection failure (network unreachable, fingerprint mismatch)
 * still throws, same as [PeerQuicClient.connect].
 */
@Throws(IOException::class)
fun connectToServer(
    device: SwarmDevice,
    clientCertificate: X509Certificate,
    clientKey: PrivateKey,
    connectTimeout: Duration = Duration.ofSeconds(5),
): PeerQuicClient? {
    val address = device.metadata["peer_addr"]?.let(PeerAddress::parse) ?: return null
    return PeerQuicClient.connect(address.host, address.port, clientCertificate, clientKey, device.certFingerprint, connectTimeout)
}
