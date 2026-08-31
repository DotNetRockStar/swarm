package app.swarm.tv.core.client

import app.swarm.tv.core.rest.DeviceRegistration
import app.swarm.tv.core.rest.DeviceType
import app.swarm.tv.core.rest.ActivationStatus
import kotlinx.coroutines.test.runTest
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.jupiter.api.AfterEach
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertInstanceOf
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.BeforeEach
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.assertThrows

class StunApiClientTest {
    private lateinit var server: MockWebServer
    private lateinit var client: StunApiClient

    @BeforeEach
    fun setUp() {
        server = MockWebServer()
        server.start()
        client = StunApiClient(server.url("/").toString())
    }

    @AfterEach
    fun tearDown() {
        server.shutdown()
    }

    private fun sampleDevice() = DeviceRegistration(
        name = "Test Server",
        deviceType = DeviceType.SERVER,
        machineId = "abc123",
        certFingerprint = "ab".repeat(32),
        platform = "test",
        appVersion = "0.1.0",
    )

    @Test
    fun `register device success`() = runTest {
        server.enqueue(
            MockResponse()
                .setResponseCode(201)
                .setBody("""{"access_token":"tok","device_id":"dev-1","swarm":{"id":"sw-1","name":"Home"}}"""),
        )
        val response = client.registerDevice("12345678", sampleDevice())
        assertEquals("tok", response.accessToken)
        assertEquals("Home", response.swarm.name)

        val recorded = server.takeRequest()
        assertEquals("POST", recorded.method)
        assertEquals("/api/v1/devices/register", recorded.path)
        assertTrue(recorded.body.readUtf8().contains("\"cert_fingerprint\""))
    }

    @Test
    fun `register device maps api error and flags unauthorized`() = runTest {
        server.enqueue(
            MockResponse()
                .setResponseCode(401)
                .setBody("""{"code":"unauthorized","message":"join code is invalid, expired, or already used"}"""),
        )
        val error = assertThrows<StunClientError> { client.registerDevice("00000000", sampleDevice()) }
        val apiError = assertInstanceOf(StunClientError.Api::class.java, error)
        assertEquals("unauthorized", apiError.code)
        assertTrue(apiError.isUnauthorized)
    }

    @Test
    fun `create activation posts device and reuses existing bearer token`() = runTest {
        server.enqueue(
            MockResponse()
                .setResponseCode(201)
                .setBody(
                    """{"activation_id":"act-1","code":"12345678","poll_token":"poll-token","access_token":"access-token","expires_at":"2026-08-31T12:00:00Z"}""",
                ),
        )

        val response = client.createActivation(sampleDevice(), "existing-token")
        assertEquals("act-1", response.activationId)
        assertEquals("12345678", response.code)

        val recorded = server.takeRequest()
        assertEquals("POST", recorded.method)
        assertEquals("/api/v1/activations", recorded.path)
        assertEquals("Bearer existing-token", recorded.getHeader("Authorization"))
        assertTrue(recorded.body.readUtf8().contains("\"device\""))
    }

    @Test
    fun `activation status polls with poll token and decodes approval`() = runTest {
        server.enqueue(
            MockResponse()
                .setResponseCode(200)
                .setBody(
                    """{"status":"approved","device_id":"dev-1","swarm":{"id":"sw-1","name":"Home"},"expires_at":"2026-08-31T12:00:00Z"}""",
                ),
        )

        val response = client.activationStatus("act-1", "poll-token")
        assertEquals(ActivationStatus.APPROVED, response.status)
        assertEquals("dev-1", response.deviceId)
        assertEquals("Home", response.swarm?.name)

        val recorded = server.takeRequest()
        assertEquals("GET", recorded.method)
        assertEquals("/api/v1/activations/act-1", recorded.path)
        assertEquals("Bearer poll-token", recorded.getHeader("Authorization"))
    }

    @Test
    fun `swarm devices sends bearer token`() = runTest {
        server.enqueue(
            MockResponse()
                .setResponseCode(200)
                .setBody("""{"swarm":{"id":"sw-1","name":"Home"},"devices":[]}"""),
        )
        val response = client.swarmDevices("secret-token", "sw-1")
        assertEquals("Home", response.swarm.name)
        assertTrue(response.devices.isEmpty())

        val recorded = server.takeRequest()
        assertEquals("Bearer secret-token", recorded.getHeader("Authorization"))
        assertEquals("/api/v1/swarms/sw-1/devices", recorded.path)
    }

    @Test
    fun `join swarm posts the code with bearer auth`() = runTest {
        server.enqueue(MockResponse().setResponseCode(200).setBody("""{"id":"sw-2","name":"Office"}"""))
        val swarm = client.joinSwarm("secret-token", "87654321")
        assertEquals("Office", swarm.name)

        val recorded = server.takeRequest()
        assertEquals("POST", recorded.method)
        assertEquals("/api/v1/swarms/join", recorded.path)
        assertEquals("Bearer secret-token", recorded.getHeader("Authorization"))
        assertTrue(recorded.body.readUtf8().contains("87654321"))
    }

    @Test
    fun `unreachable server is a network error not an api error`() = runTest {
        server.shutdown() // nothing listening
        val error = assertThrows<StunClientError> { client.registerDevice("12345678", sampleDevice()) }
        assertInstanceOf(StunClientError.Network::class.java, error)
        assertFalse(error.isUnauthorized)
    }

    @Test
    fun `malformed response body is a decode error`() = runTest {
        server.enqueue(MockResponse().setResponseCode(200).setBody("not json"))
        val error = assertThrows<StunClientError> { client.swarmDevices("token", "sw-1") }
        assertInstanceOf(StunClientError.Decode::class.java, error)
    }
}
