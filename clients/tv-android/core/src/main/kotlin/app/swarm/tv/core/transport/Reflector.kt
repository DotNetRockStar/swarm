/**
 * Reflexive-address discovery client, mirroring
 * `swarm_p2p::reflector::reflexive_addr` (Rust): send `bind` to
 * `apps/stun-server`'s UDP reflector, get back the address it observed us
 * at. Lives alongside [PeerQuicClient] (not the `client` package's
 * REST/signaling types) for the same reason the Rust version lives in
 * `swarm-p2p` rather than `swarm-stun-client` — this is P2P connectivity
 * self-discovery, not STUN communication.
 */
package app.swarm.tv.core.transport

import app.swarm.tv.core.rest.SwarmJson
import app.swarm.tv.core.signal.REFLECTOR_BIND_REQUEST
import app.swarm.tv.core.signal.ReflectorResponse
import java.io.IOException
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.net.SocketTimeoutException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.decodeFromString

/**
 * A single request/response is expected to be near-instant on a healthy
 * path; retry policy (try the fallback reflector port, try again) is a
 * concern for whatever calls this, not this primitive — same split as the
 * Rust version.
 */
private const val REFLECTOR_TIMEOUT_MS = 2000

sealed class ReflectorError(message: String) : Exception(message) {
    class Network(cause: Throwable) : ReflectorError("could not reach the reflector: ${cause.message}")
    data object Timeout : ReflectorError("reflector did not respond within the timeout")
    class Decode(reason: String) : ReflectorError("could not parse the reflector's response: $reason")
}

/**
 * Sends `bind` to [reflectorAddr] on [socket] and returns the address the
 * reflector observed us at. Takes a socket the caller already owns, rather
 * than opening a fresh one internally, so the same local port can be reused
 * for the actual hole punch afterward — the reflexive mapping a NAT hands
 * out is only valid for the 4-tuple it was observed on.
 */
suspend fun reflexiveAddr(socket: DatagramSocket, reflectorAddr: InetSocketAddress): InetSocketAddress =
    withContext(Dispatchers.IO) {
        try {
            socket.send(DatagramPacket(REFLECTOR_BIND_REQUEST, REFLECTOR_BIND_REQUEST.size, reflectorAddr))
        } catch (e: IOException) {
            throw ReflectorError.Network(e)
        }

        val buffer = ByteArray(512)
        val response = DatagramPacket(buffer, buffer.size)
        val originalTimeout = socket.soTimeout
        try {
            socket.soTimeout = REFLECTOR_TIMEOUT_MS
            socket.receive(response)
        } catch (e: SocketTimeoutException) {
            throw ReflectorError.Timeout
        } catch (e: IOException) {
            throw ReflectorError.Network(e)
        } finally {
            socket.soTimeout = originalTimeout
        }

        // Cheap sanity check, not real authentication: UDP has no session,
        // so don't trust the first datagram to land on this socket unless
        // it at least claims to be from the address we queried.
        val from = response.socketAddress as InetSocketAddress
        if (from.address != reflectorAddr.address || from.port != reflectorAddr.port) {
            throw ReflectorError.Decode("reply from unexpected address $from")
        }

        val parsed = try {
            SwarmJson.decodeFromString<ReflectorResponse>(String(response.data, 0, response.length, Charsets.UTF_8))
        } catch (e: Exception) {
            throw ReflectorError.Decode(e.message ?: "malformed response")
        }
        InetSocketAddress(parsed.ip, parsed.port)
    }
