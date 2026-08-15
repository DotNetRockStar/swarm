/**
 * Cross-language wire-compatibility checks. The JSON fixture strings below
 * are not hand-guessed — they were captured by actually running
 * `serde_json::to_string` on equivalent Rust values from `swarm-core::rest`
 * (see the shell history for the throwaway `fixture-printer` binary used to
 * generate them). If `swarm-core::rest` changes shape, regenerate these
 * fixtures the same way rather than editing them by hand.
 */
package app.swarm.tv.core.rest

import kotlinx.serialization.decodeFromString
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class ContractsTest {

    @Test
    fun `kotlin decodes a real serde-produced register response`() {
        val json = """{"access_token":"tok-abc123","device_id":"dev-1","swarm":{"id":"sw-1","name":"Home"}}"""
        val response = SwarmJson.decodeFromString<RegisterDeviceResponse>(json)
        assertEquals("tok-abc123", response.accessToken)
        assertEquals("dev-1", response.deviceId)
        assertEquals(SwarmSummary("sw-1", "Home"), response.swarm)
    }

    @Test
    fun `kotlin decodes a real serde-produced swarm devices response`() {
        val json = """{"swarm":{"id":"sw-1","name":"Home"},"devices":[{"device_id":"dev-2","name":"Media Server",""" +
            """"device_type":"server","cert_fingerprint":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd",""" +
            """"online":true,"last_seen_at":"2026-08-15T00:00:00Z","metadata":{"hostname":"mac-mini"}}]}"""
        val response = SwarmJson.decodeFromString<SwarmDevicesResponse>(json)
        assertEquals(1, response.devices.size)
        val device = response.devices[0]
        assertEquals("dev-2", device.deviceId)
        assertEquals(DeviceType.SERVER, device.deviceType)
        assertTrue(device.online)
        assertEquals("2026-08-15T00:00:00Z", device.lastSeenAt)
        assertEquals(mapOf("hostname" to "mac-mini"), device.metadata)
    }

    @Test
    fun `swarm device with a missing last_seen_at decodes to null`() {
        // Rust always emits this field, but a client must also tolerate a
        // response-extensibility future where it's omitted (contract
        // discipline: responses are extensible, per swarm-core::rest docs).
        val json = """{"device_id":"d","name":"n","device_type":"client","cert_fingerprint":"ab","online":false}"""
        val device = SwarmJson.decodeFromString<SwarmDevice>(json)
        assertNull(device.lastSeenAt)
        assertEquals(emptyMap<String, String>(), device.metadata)
    }

    @Test
    fun `kotlin decodes a real serde-produced api error`() {
        val json = """{"code":"unauthorized","message":"bad token"}"""
        val error = SwarmJson.decodeFromString<ApiError>(json)
        assertEquals("unauthorized", error.code)
        assertEquals("bad token", error.message)
    }

    @Test
    fun `device type wire values are lowercase snake tags not enum names`() {
        assertEquals(""""client"""", SwarmJson.encodeToString(DeviceType.CLIENT))
        assertEquals(""""server"""", SwarmJson.encodeToString(DeviceType.SERVER))
        assertEquals(""""both"""", SwarmJson.encodeToString(DeviceType.BOTH))
    }

    @Test
    fun `kotlin-encoded register request has exactly rust's expected key set`() {
        val request = RegisterDeviceRequest(
            code = "12345678",
            device = DeviceRegistration(
                name = "Living Room TV",
                deviceType = DeviceType.CLIENT,
                machineId = "a1b2c3",
                certFingerprint = "ab".repeat(32),
                platform = "firetv-api25",
                appVersion = "0.1.0",
                metadata = mapOf("model" to "AFTKA"),
            ),
        )
        val encoded = SwarmJson.encodeToString(request)
        // The real serde_json output for the equivalent Rust value, captured
        // from swarm-core — full string equality proves both field order and
        // key naming match exactly, not just "decodes to the same value".
        val expected = """{"code":"12345678","device":{"name":"Living Room TV","device_type":"client",""" +
            """"machine_id":"a1b2c3","cert_fingerprint":"${"ab".repeat(32)}","platform":"firetv-api25",""" +
            """"app_version":"0.1.0","metadata":{"model":"AFTKA"}}}"""
        assertEquals(expected, encoded)

        // deny_unknown_fields on the Rust side means the key set must be
        // exact — round-trip through a raw JsonObject and check nothing
        // extra snuck in via a naming-strategy surprise.
        val keys = Json.parseToJsonElement(encoded).jsonObject.keys
        assertEquals(setOf("code", "device"), keys)
    }

    @Test
    fun `register request round trips through kotlin`() {
        val request = RegisterDeviceRequest(
            code = "87654321",
            device = DeviceRegistration(
                name = "Media Server",
                deviceType = DeviceType.BOTH,
                machineId = "xyz",
                certFingerprint = "cd".repeat(32),
                platform = "macos-aarch64",
                appVersion = "0.1.0",
            ),
        )
        val decoded = SwarmJson.decodeFromString<RegisterDeviceRequest>(SwarmJson.encodeToString(request))
        assertEquals(request, decoded)
    }
}
