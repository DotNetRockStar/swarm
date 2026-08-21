/**
 * Device <-> STUN REST contracts. Hand-mirrored from `swarm-core::rest`
 * (Rust) — kept field-for-field identical to the JSON that crate produces
 * and consumes, so this file changing is the signal to re-check that crate
 * (and vice versa). [SwarmJson]'s `JsonNamingStrategy.SnakeCase` reproduces
 * serde's `rename_all = "snake_case"` for every property; enum wire values
 * need an explicit `@SerialName` per entry since the naming strategy does
 * not touch enum entries.
 */
package app.swarm.tv.core.rest

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

@Serializable
enum class DeviceType {
    @SerialName("client") CLIENT,
    @SerialName("server") SERVER,
    @SerialName("both") BOTH,
}

@Serializable
data class DeviceRegistration(
    val name: String,
    val deviceType: DeviceType,
    val machineId: String,
    /** Lowercase hex SHA-256 of the device's self-signed certificate (DER). */
    val certFingerprint: String,
    val platform: String,
    val appVersion: String,
    val metadata: Map<String, String> = emptyMap(),
)

@Serializable
data class RegisterDeviceRequest(
    val code: String,
    val device: DeviceRegistration,
)

@Serializable
data class SwarmSummary(
    val id: String,
    val name: String,
)

@Serializable
data class RegisterDeviceResponse(
    val accessToken: String,
    val deviceId: String,
    val swarm: SwarmSummary,
)

@Serializable
data class CreateActivationRequest(val device: DeviceRegistration)

@Serializable
data class CreateActivationResponse(
    val activationId: String,
    val code: String,
    val pollToken: String,
    val accessToken: String,
    val expiresAt: String,
)

@Serializable
enum class ActivationStatus {
    @SerialName("pending") PENDING,
    @SerialName("approved") APPROVED,
    @SerialName("expired") EXPIRED,
}

@Serializable
data class ActivationStatusResponse(
    val status: ActivationStatus,
    val deviceId: String? = null,
    val swarm: SwarmSummary? = null,
    val expiresAt: String,
)

@Serializable
data class JoinSwarmRequest(
    val code: String,
)

@Serializable
data class SwarmDevice(
    val deviceId: String,
    val name: String,
    val deviceType: DeviceType,
    val certFingerprint: String,
    val online: Boolean,
    val lastSeenAt: String? = null,
    val metadata: Map<String, String> = emptyMap(),
)

@Serializable
data class SwarmDevicesResponse(
    val swarm: SwarmSummary,
    val devices: List<SwarmDevice>,
)

@Serializable
data class ApiError(
    val code: String,
    val message: String,
)
