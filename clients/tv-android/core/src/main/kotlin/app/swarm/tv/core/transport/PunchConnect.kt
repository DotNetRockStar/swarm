/**
 * Ties every hole-punch primitive together into one "connect to this peer"
 * flow, mirroring `apps/server/src/punch_connect.rs`'s
 * `initiate_punch_connection` (Rust) — but only the "I" (initiator) role.
 * The Fire TV client only ever dials out to servers, never accepts an
 * inbound connection, so it doesn't need the "R" (responder) role Rust's
 * `ServerCore` has — a smaller, more scoped port than the Rust side needed.
 */
package app.swarm.tv.core.transport

import app.swarm.tv.core.client.SignalingClient
import app.swarm.tv.core.signal.Candidate
import app.swarm.tv.core.signal.CandidateKind
import app.swarm.tv.core.signal.SignalMessage
import app.swarm.tv.core.signal.SignalPayload
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.security.PrivateKey
import java.security.cert.X509Certificate
import kotlin.random.Random
import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.coroutines.withTimeoutOrNull

private const val ANSWER_TIMEOUT_MS = 10_000L
private const val CONFIRMATION_TIMEOUT_MS = 10_000L

sealed class PunchConnectError(message: String) : Exception(message) {
    data object AnswerTimeout : PunchConnectError("timed out waiting for the peer's answer")
    data object ConfirmationTimeout : PunchConnectError("timed out waiting for mutual punch confirmation")
    data object SignalingClosed : PunchConnectError("signaling connection closed mid-attempt")
    class PeerError(val code: String, val serverMessage: String) :
        PunchConnectError("peer reported a signaling error ($code): $serverMessage")
    class FingerprintMismatch(val expected: String, val actual: String) : PunchConnectError(
        "peer's answer certificate fingerprint didn't match the pinned roster entry (expected $expected, got $actual)",
    )
    data object NoCandidates : PunchConnectError("no usable candidates to punch toward")
}

/**
 * The "I" (initiator) role: offers, waits for an answer pinned to
 * [expectedFingerprint], punches to the answered candidates, confirms
 * mutually, and dials QUIC over the punched socket via
 * [PeerQuicClient.connect]'s `localSocketPort` (see its doc comment for
 * why that — not a literal socket handoff — is what kwik allows).
 */
suspend fun initiatePunchConnection(
    signaling: SignalingClient,
    signalRx: ReceiveChannel<SignalMessage>,
    reflectorAddr: InetSocketAddress,
    peerDeviceId: String,
    ownFingerprint: String,
    clientCertificate: X509Certificate,
    clientKey: PrivateKey,
    expectedFingerprint: String,
): PeerQuicClient {
    val socket = DatagramSocket(0)
    val punchId = randomPunchId()
    var succeeded = false
    try {
        val candidates = gatherCandidates(socket, reflectorAddr)
        signaling.sendSignal(peerDeviceId, SignalPayload.Offer(punchId, candidates, ownFingerprint))

        val answer = awaitSignalPayload(signalRx, peerDeviceId, ANSWER_TIMEOUT_MS, PunchConnectError.AnswerTimeout) { payload ->
            (payload as? SignalPayload.Answer)?.takeIf { it.punchId == punchId }
        }
        if (answer.certFingerprint != expectedFingerprint) {
            throw PunchConnectError.FingerprintMismatch(expectedFingerprint, answer.certFingerprint)
        }

        val targets = candidateAddrs(answer.candidates)
        if (targets.isEmpty()) throw PunchConnectError.NoCandidates
        val confirmedAddr = punch(socket, targets)
        signaling.sendSignal(peerDeviceId, SignalPayload.Punched(punchId, true))

        awaitSignalPayload(signalRx, peerDeviceId, CONFIRMATION_TIMEOUT_MS, PunchConnectError.ConfirmationTimeout) { payload ->
            (payload as? SignalPayload.Punched)?.takeIf { it.punchId == punchId && it.ok }
        }

        val heldPort = socket.localPort
        socket.close()
        succeeded = true
        return PeerQuicClient.connect(
            confirmedAddr.address.hostAddress,
            confirmedAddr.port,
            clientCertificate,
            clientKey,
            expectedFingerprint,
            localSocketPort = heldPort,
        )
    } finally {
        if (!succeeded) socket.close()
    }
}

private fun randomPunchId(): String {
    val bytes = ByteArray(16)
    Random.nextBytes(bytes)
    return bytes.joinToString("") { "%02x".format(it) }
}

private suspend fun gatherCandidates(socket: DatagramSocket, reflectorAddr: InetSocketAddress): List<Candidate> {
    val candidates = mutableListOf<Candidate>()
    val local = detectLocalIpv4()
    candidates += Candidate(CandidateKind.LAN, local.hostAddress, socket.localPort)
    runCatching { reflexiveAddr(socket, reflectorAddr) }.getOrNull()?.let { reflexive ->
        candidates += Candidate(CandidateKind.REFLEXIVE, reflexive.address.hostAddress, reflexive.port)
    }
    return candidates
}

/** LAN candidates first, matching the protocol doc's punch ordering. */
private fun candidateAddrs(candidates: List<Candidate>): List<InetSocketAddress> {
    val order = mapOf(CandidateKind.LAN to 0, CandidateKind.FORWARDED to 1, CandidateKind.REFLEXIVE to 2)
    return candidates.sortedBy { order.getValue(it.kind) }.mapNotNull { candidate ->
        runCatching { InetSocketAddress(candidate.ip, candidate.port) }.getOrNull()
    }
}

/**
 * Waits for a `Signal` from [fromDevice] whose payload [matches] accepts,
 * ignoring anything else (presence, signals from other peers, non-matching
 * payloads) that arrives in the meantime — this attempt only cares about
 * its own negotiation. An `Error` frame is treated as fatal immediately.
 */
private suspend fun <T> awaitSignalPayload(
    rx: ReceiveChannel<SignalMessage>,
    fromDevice: String,
    timeoutMs: Long,
    onTimeout: PunchConnectError,
    matches: (SignalPayload) -> T?,
): T = withTimeoutOrNull(timeoutMs) {
    while (true) {
        val message = rx.receiveCatching().getOrNull() ?: throw PunchConnectError.SignalingClosed
        when (message) {
            is SignalMessage.Signal -> if (message.from == fromDevice) {
                matches(message.payload)?.let { return@withTimeoutOrNull it }
            }
            is SignalMessage.Error -> throw PunchConnectError.PeerError(message.code, message.message)
            else -> {}
        }
    }
    error("unreachable: the loop above only exits via return or throw")
} ?: throw onTimeout
