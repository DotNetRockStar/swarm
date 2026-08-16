/**
 * Device <-> STUN signaling messages, hand-mirrored from `swarm-core::signal`
 * (Rust) — carried as JSON text frames over the persistent WSS connection
 * (`/api/v1/ws`). Every shape here was checked against real
 * `serde_json::to_string` output (see `ContractsTest.kt`), not guessed —
 * see the `ByteRange` note elsewhere in this codebase for why that
 * discipline matters: `SignalMessage`'s discriminator (`type`) and
 * `SignalPayload`'s (`kind`) are both *adjacently* tagged (fields flattened
 * alongside the tag in one JSON object), which is kotlinx.serialization's
 * own default sealed-class shape — unlike `ByteRange`'s externally-tagged
 * enum, these need no hand-written serializer, just `@SerialName` per
 * variant plus `@JsonClassDiscriminator` where the key isn't the default
 * `"type"`.
 */
@file:OptIn(ExperimentalSerializationApi::class)

package app.swarm.tv.core.signal

import app.swarm.tv.core.capability.CapabilityProfile
import app.swarm.tv.core.rest.DeviceType
import kotlinx.serialization.ExperimentalSerializationApi
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonClassDiscriminator

@Serializable
sealed class SignalMessage {
    /** First message after the socket opens; authenticates the connection. */
    @Serializable
    @SerialName("hello")
    data class Hello(
        val protocolVersion: Int,
        val accessToken: String,
        val deviceId: String,
        val capabilities: CapabilityProfile? = null,
    ) : SignalMessage()

    /**
     * Server accepts the hello. [observedAddr] is this device's public
     * address as seen by the STUN server (TCP-derived); the UDP reflexive
     * address still comes from a separate reflector query.
     */
    @Serializable
    @SerialName("hello_ack")
    data class HelloAck(
        val sessionId: String,
        val observedAddr: String,
        val reflectorPorts: List<Int>,
    ) : SignalMessage()

    @Serializable
    @SerialName("ping")
    data class Ping(val seq: Long) : SignalMessage()

    @Serializable
    @SerialName("pong")
    data class Pong(val seq: Long) : SignalMessage()

    /** Pushed by the server to all online devices sharing a swarm whenever a peer's state changes. */
    @Serializable
    @SerialName("presence")
    data class Presence(
        val deviceId: String,
        val deviceType: DeviceType,
        val online: Boolean,
        val swarmIds: List<String>,
        val streaming: StreamingStatus? = null,
    ) : SignalMessage()

    /**
     * Relayed verbatim between two devices sharing a swarm; the server fills
     * in [from] and enforces shared-swarm membership. Carries hole-punch
     * negotiation payloads — the STUN server never opens them beyond routing.
     */
    @Serializable
    @SerialName("signal")
    data class Signal(
        val from: String? = null,
        val to: String,
        val payload: SignalPayload,
    ) : SignalMessage()

    /** Graceful shutdown notice from either side. */
    @Serializable
    @SerialName("bye")
    data object Bye : SignalMessage()

    @Serializable
    @SerialName("error")
    data class Error(val code: String, val message: String) : SignalMessage()
}

/** Server-app load advertisement, folded into presence so clients can score sources by transcode headroom. */
@Serializable
data class StreamingStatus(
    val transcodeCapacity: Int,
    val activeSessions: Int,
    val hwAccel: Boolean,
)

@Serializable
@JsonClassDiscriminator("kind")
sealed class SignalPayload {
    /** Connection offer: my candidates + my cert fingerprint (re-confirming the registration-time pin) + a random punch session id. */
    @Serializable
    @SerialName("offer")
    data class Offer(
        val punchId: String,
        val candidates: List<Candidate>,
        val certFingerprint: String,
    ) : SignalPayload()

    @Serializable
    @SerialName("answer")
    data class Answer(
        val punchId: String,
        val candidates: List<Candidate>,
        val certFingerprint: String,
    ) : SignalPayload()

    /** Mutual confirmation that PUNCH_MAGIC traffic arrived, so both sides switch to the punched 4-tuple together before the QUIC handshake. */
    @Serializable
    @SerialName("punched")
    data class Punched(val punchId: String, val ok: Boolean) : SignalPayload()
}

@Serializable
data class Candidate(
    val kind: CandidateKind,
    val ip: String,
    val port: Int,
)

@Serializable
enum class CandidateKind {
    /** Local interface address — same-LAN peers connect directly. */
    @SerialName("lan") LAN,
    /** Reflexive address learned from the UDP reflector. */
    @SerialName("reflexive") REFLEXIVE,
    /** UPnP/NAT-PMP-mapped or manually forwarded port — reachable without punching. */
    @SerialName("forwarded") FORWARDED,
}

/** Datagram a device sends to the UDP reflector. The reflector replies with JSON `{"ip": "...", "port": ...}`. */
val REFLECTOR_BIND_REQUEST: ByteArray = "bind".toByteArray(Charsets.US_ASCII)

@Serializable
data class ReflectorResponse(val ip: String, val port: Int)

/** Datagram body used during simultaneous hole punching. */
val PUNCH_MAGIC: ByteArray = "swarm-punch-v1".toByteArray(Charsets.US_ASCII)

/** Mirrors `swarm_core::PROTOCOL_VERSION` (Rust) — travels in every `hello`. */
const val PROTOCOL_VERSION: Int = 1
