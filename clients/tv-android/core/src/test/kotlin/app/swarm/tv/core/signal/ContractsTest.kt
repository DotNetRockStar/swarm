/**
 * Every fixture below is real `serde_json::to_string` output captured by
 * running a throwaway `cargo run --example fixture_printer -p swarm-core`
 * against the actual Rust types (now deleted) — not hand-guessed JSON, the
 * same discipline `rest/ContractsTest.kt` and `peer/ContractsTest.kt` use.
 * Each fixture is checked both ways: decodes to the expected Kotlin value,
 * and the Kotlin value re-encodes back to the exact same string.
 */
package app.swarm.tv.core.signal

import app.swarm.tv.core.capability.CapabilityProfile
import app.swarm.tv.core.rest.DeviceType
import app.swarm.tv.core.rest.SwarmJson
import kotlinx.serialization.decodeFromString
import kotlinx.serialization.encodeToString
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Test

class ContractsTest {
    private fun roundtrip(value: SignalMessage, json: String) {
        assertEquals(value, SwarmJson.decodeFromString<SignalMessage>(json))
        assertEquals(json, SwarmJson.encodeToString(value))
    }

    @Test
    fun `hello with capabilities`() {
        roundtrip(
            SignalMessage.Hello(
                protocolVersion = 1,
                accessToken = "tok",
                deviceId = "dev-1",
                capabilities = CapabilityProfile.fireTvBaseline(),
            ),
            """{"type":"hello","protocol_version":1,"access_token":"tok","device_id":"dev-1","capabilities":{"containers":["mp4","hls"],"video_codecs":["h264:high@4.2"],"audio_codecs":["aac","ac3","mp3"],"max_width":1920,"max_height":1080,"max_bitrate":12000000,"hdr":false}}""",
        )
    }

    @Test
    fun `hello without capabilities omits the field`() {
        roundtrip(
            SignalMessage.Hello(protocolVersion = 1, accessToken = "tok", deviceId = "dev-1"),
            """{"type":"hello","protocol_version":1,"access_token":"tok","device_id":"dev-1"}""",
        )
    }

    @Test
    fun `hello_ack`() {
        roundtrip(
            SignalMessage.HelloAck(sessionId = "s", observedAddr = "203.0.113.9:52011", reflectorPorts = listOf(443, 3478)),
            """{"type":"hello_ack","session_id":"s","observed_addr":"203.0.113.9:52011","reflector_ports":[443,3478]}""",
        )
    }

    @Test
    fun `ping and pong`() {
        roundtrip(SignalMessage.Ping(seq = 7), """{"type":"ping","seq":7}""")
        roundtrip(SignalMessage.Pong(seq = 7), """{"type":"pong","seq":7}""")
    }

    @Test
    fun `presence with streaming`() {
        roundtrip(
            SignalMessage.Presence(
                deviceId = "dev-2",
                deviceType = DeviceType.SERVER,
                online = true,
                swarmIds = listOf("sw-1"),
                streaming = StreamingStatus(transcodeCapacity = 2, activeSessions = 1, hwAccel = true),
            ),
            """{"type":"presence","device_id":"dev-2","device_type":"server","online":true,"swarm_ids":["sw-1"],"streaming":{"transcode_capacity":2,"active_sessions":1,"hw_accel":true}}""",
        )
    }

    @Test
    fun `presence without streaming omits the field but keeps empty swarm_ids`() {
        roundtrip(
            SignalMessage.Presence(deviceId = "dev-2", deviceType = DeviceType.CLIENT, online = false, swarmIds = emptyList()),
            """{"type":"presence","device_id":"dev-2","device_type":"client","online":false,"swarm_ids":[]}""",
        )
    }

    @Test
    fun `signal offer with a from`() {
        roundtrip(
            SignalMessage.Signal(
                from = "dev-1",
                to = "dev-2",
                payload = SignalPayload.Offer(
                    punchId = "p1",
                    candidates = listOf(
                        Candidate(CandidateKind.LAN, "192.168.1.10", 40000),
                        Candidate(CandidateKind.REFLEXIVE, "203.0.113.9", 61234),
                        Candidate(CandidateKind.FORWARDED, "203.0.113.9", 8543),
                    ),
                    certFingerprint = "ab".repeat(32),
                ),
            ),
            """{"type":"signal","from":"dev-1","to":"dev-2","payload":{"kind":"offer","punch_id":"p1","candidates":[{"kind":"lan","ip":"192.168.1.10","port":40000},{"kind":"reflexive","ip":"203.0.113.9","port":61234},{"kind":"forwarded","ip":"203.0.113.9","port":8543}],"cert_fingerprint":"abababababababababababababababababababababababababababababababab"}}""",
        )
    }

    @Test
    fun `signal answer without a from omits the field`() {
        roundtrip(
            SignalMessage.Signal(
                to = "dev-2",
                payload = SignalPayload.Answer(punchId = "p1", candidates = emptyList(), certFingerprint = "cd".repeat(32)),
            ),
            """{"type":"signal","to":"dev-2","payload":{"kind":"answer","punch_id":"p1","candidates":[],"cert_fingerprint":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"}}""",
        )
    }

    @Test
    fun `signal punched`() {
        roundtrip(
            SignalMessage.Signal(from = "dev-2", to = "dev-1", payload = SignalPayload.Punched(punchId = "p1", ok = true)),
            """{"type":"signal","from":"dev-2","to":"dev-1","payload":{"kind":"punched","punch_id":"p1","ok":true}}""",
        )
    }

    @Test
    fun `bye`() {
        roundtrip(SignalMessage.Bye, """{"type":"bye"}""")
    }

    @Test
    fun `error`() {
        roundtrip(
            SignalMessage.Error(code = "unauthorized", message = "bad token"),
            """{"type":"error","code":"unauthorized","message":"bad token"}""",
        )
    }

    @Test
    fun `reflector response`() {
        val fixture = """{"ip":"203.0.113.9","port":61234}"""
        val value = ReflectorResponse(ip = "203.0.113.9", port = 61234)
        assertEquals(value, SwarmJson.decodeFromString<ReflectorResponse>(fixture))
        assertEquals(fixture, SwarmJson.encodeToString(value))
    }

    @Test
    fun `capability profile`() {
        val fixture =
            """{"containers":["mp4","hls"],"video_codecs":["h264:high@4.2"],"audio_codecs":["aac","ac3","mp3"],"max_width":1920,"max_height":1080,"max_bitrate":12000000,"hdr":false}"""
        val value = CapabilityProfile.fireTvBaseline()
        assertEquals(value, SwarmJson.decodeFromString<CapabilityProfile>(fixture))
        assertEquals(fixture, SwarmJson.encodeToString(value))
    }

    @Test
    fun `punch magic and reflector bind request match the rust byte constants`() {
        assertEquals("swarm-punch-v1", String(PUNCH_MAGIC, Charsets.US_ASCII))
        assertEquals("bind", String(REFLECTOR_BIND_REQUEST, Charsets.US_ASCII))
    }
}
