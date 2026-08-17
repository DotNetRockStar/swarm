/**
 * Simultaneous UDP hole punching, mirroring `swarm_p2p::punch::punch`
 * (Rust): both sides blast `PUNCH_MAGIC` at every candidate address, LAN
 * first, while listening on the same socket for the same magic coming back
 * the other way.
 *
 * Deliberately signaling-agnostic, same as the Rust version: [punch] only
 * proves *this device* can receive from one of `candidates` — it knows
 * nothing about `SignalingClient`. Mutual confirmation (so neither side
 * switches to the punched path while the other is still waiting, per
 * `docs/PROTOCOL.md`) is whatever orchestrates this one layer up.
 */
package app.swarm.tv.core.transport

import app.swarm.tv.core.signal.PUNCH_MAGIC
import java.io.IOException
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.net.SocketTimeoutException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

const val PUNCH_MAX_ATTEMPTS = 20
const val PUNCH_ATTEMPT_INTERVAL_MS = 200L

sealed class PunchError(message: String) : Exception(message) {
    class NoResponse(candidateCount: Int, attempts: Int) :
        PunchError("no response from any of $candidateCount candidate(s) after $attempts attempts")
    class Network(cause: Throwable) : PunchError("socket error while punching: ${cause.message}")
}

/**
 * Tries every address in [candidates] (in the given order, each round —
 * ordering LAN-first is the caller's job) every [PUNCH_ATTEMPT_INTERVAL_MS],
 * up to [PUNCH_MAX_ATTEMPTS] times, while listening on [socket] for the same
 * magic coming back. Returns the source address of the first valid magic
 * packet received from a listed candidate, having already sent one more
 * magic packet back to that exact address on the way out — the scheduled
 * blast for this round may have gone out to a different candidate that
 * didn't pan out, so this makes sure a packet flows back down the
 * *confirmed* path immediately rather than waiting for the next round.
 */
suspend fun punch(socket: DatagramSocket, candidates: List<InetSocketAddress>): InetSocketAddress =
    withContext(Dispatchers.IO) {
        if (candidates.isEmpty()) throw PunchError.NoResponse(0, 0)
        val originalTimeout = socket.soTimeout
        try {
            repeat(PUNCH_MAX_ATTEMPTS) {
                for (candidate in candidates) sendMagic(socket, candidate)
                receiveMagicWithin(socket, candidates, PUNCH_ATTEMPT_INTERVAL_MS)?.let { from ->
                    sendMagic(socket, from) // best-effort immediate reply
                    return@withContext from
                }
            }
            throw PunchError.NoResponse(candidates.size, PUNCH_MAX_ATTEMPTS)
        } finally {
            socket.soTimeout = originalTimeout
        }
    }

private fun sendMagic(socket: DatagramSocket, to: InetSocketAddress) {
    try {
        socket.send(DatagramPacket(PUNCH_MAGIC, PUNCH_MAGIC.size, to))
    } catch (e: IOException) {
        throw PunchError.Network(e)
    }
}

/**
 * Listens for up to [windowMs], returning the source address of the first
 * valid magic packet from a listed candidate, or null if the window elapses
 * first. A stray or malformed datagram doesn't end the wait, same as the
 * Rust version — it just keeps listening out the rest of the window.
 */
private fun receiveMagicWithin(socket: DatagramSocket, candidates: List<InetSocketAddress>, windowMs: Long): InetSocketAddress? {
    val buffer = ByteArray(PUNCH_MAGIC.size)
    val deadline = System.nanoTime() + windowMs * 1_000_000
    while (true) {
        val remainingMs = (deadline - System.nanoTime()) / 1_000_000
        if (remainingMs <= 0) return null
        socket.soTimeout = remainingMs.toInt().coerceAtLeast(1)
        val packet = DatagramPacket(buffer, buffer.size)
        try {
            socket.receive(packet)
        } catch (e: SocketTimeoutException) {
            return null
        } catch (e: IOException) {
            throw PunchError.Network(e)
        }
        val from = packet.socketAddress as InetSocketAddress
        val isValidMagic = packet.length == PUNCH_MAGIC.size && buffer.copyOf(packet.length).contentEquals(PUNCH_MAGIC)
        if (isValidMagic && candidates.contains(from)) return from
    }
}
