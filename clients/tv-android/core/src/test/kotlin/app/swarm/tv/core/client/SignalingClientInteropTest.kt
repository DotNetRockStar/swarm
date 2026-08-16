/**
 * Proves `SignalingClient` end to end against the real release
 * `swarm-stun-server` binary — the Kotlin counterpart to
 * `crates/swarm-stun-client/tests/signaling.rs`, same five cases, so the
 * two clients are held to the same bar rather than just "compiles against
 * hand-mirrored types." Skips (not fails) if the binary hasn't been built,
 * same discipline as `PeerQuicClientInteropTest`.
 */
package app.swarm.tv.core.client

import app.swarm.tv.core.rest.DeviceRegistration
import app.swarm.tv.core.rest.DeviceType
import app.swarm.tv.core.signal.SignalMessage
import app.swarm.tv.core.signal.SignalPayload
import java.io.BufferedReader
import java.io.File
import java.util.concurrent.TimeUnit
import java.util.regex.Pattern
import kotlin.random.Random
import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.junit.jupiter.api.AfterEach
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Assumptions.assumeTrue
import org.junit.jupiter.api.BeforeEach
import org.junit.jupiter.api.Test

private val jsonMediaType = "application/json".toMediaType()

class SignalingClientInteropTest {
    private var process: Process? = null

    @BeforeEach
    fun setUp() {
        assumeTrue(resolveServerBinary() != null, "swarm-stun-server release binary not found — run `cargo build --release -p stun-server --bin swarm-stun-server` first")
    }

    @AfterEach
    fun tearDown() {
        process?.destroyForcibly()?.waitFor(5, TimeUnit.SECONDS)
    }

    private fun resolveServerBinary(): File? {
        // core/ -> tv-android/ -> clients/ -> swarm/ -> target/release/...
        val candidate = File("../../../target/release/swarm-stun-server").canonicalFile
        return candidate.takeIf { it.exists() }
    }

    private fun startStunServer(): String {
        val binary = resolveServerBinary()!!
        val dbPath = File.createTempFile("swarm-signaling-interop", ".sqlite").apply { deleteOnExit() }
        val builder = ProcessBuilder(binary.absolutePath).redirectErrorStream(true)
        builder.environment().apply {
            put("SWARM_DATABASE_PATH", dbPath.absolutePath)
            put("SWARM_HTTP_BIND", "127.0.0.1:0")
            put("SWARM_REFLECTOR_PORTS", "") // not under test here — see swarm_p2p::reflector's own coverage
            put("RUST_LOG", "info")
        }
        val proc = builder.start()
        process = proc

        val reader = BufferedReader(proc.inputStream.reader())
        var bind: String? = null
        val bindPattern = Pattern.compile("bind=(\\d+\\.\\d+\\.\\d+\\.\\d+:\\d+)")
        val deadline = System.currentTimeMillis() + 15_000
        while (System.currentTimeMillis() < deadline && bind == null) {
            val rawLine = reader.readLine() ?: break
            val line = rawLine.replace(Regex("\\[[0-9;]*m"), "")
            bindPattern.matcher(line).takeIf { it.find() }?.let { bind = it.group(1) }
        }
        checkNotNull(bind) { "swarm-stun-server did not report its bind address within the deadline" }
        Thread({
            generateSequence { reader.readLine() }.forEach { println("[swarm-stun-server] $it") }
        }, "swarm-stun-server-drain").apply { isDaemon = true }.start()
        return "http://$bind"
    }

    /** Minimal cookie-session client for the STUN web API — mirrors the Rust tests' `Browser` helper. */
    private class Browser(private val base: String, private val http: OkHttpClient) {
        private lateinit var cookieHeader: String
        private lateinit var csrf: String

        fun loginFreshAccount(email: String) {
            val password = "correct horse battery"
            val registerBody = """{"email":"$email","password":"$password"}""".toRequestBody(jsonMediaType)
            http.newCall(Request.Builder().url("$base/api/v1/auth/register").post(registerBody).build()).execute().use {
                check(it.isSuccessful) { "register failed: ${it.code}" }
            }
            val loginBody = """{"email":"$email","password":"$password"}""".toRequestBody(jsonMediaType)
            http.newCall(Request.Builder().url("$base/api/v1/auth/login").post(loginBody).build()).execute().use { response ->
                check(response.isSuccessful) { "login failed: ${response.code}" }
                var session = ""
                var csrfValue = ""
                for (header in response.headers("set-cookie")) {
                    val pair = header.substringBefore(';')
                    val (name, value) = pair.split('=', limit = 2)
                    when (name) {
                        "swarm_session" -> session = value
                        "swarm_csrf" -> csrfValue = value
                    }
                }
                cookieHeader = "swarm_session=$session; swarm_csrf=$csrfValue"
                csrf = csrfValue
            }
        }

        private fun authedRequest(url: String) = Request.Builder().url(url).header("cookie", cookieHeader).header("x-swarm-csrf", csrf)

        fun createSwarm(name: String): String {
            val body = """{"name":"$name"}""".toRequestBody(jsonMediaType)
            http.newCall(authedRequest("$base/api/v1/swarms").post(body).build()).execute().use { response ->
                check(response.isSuccessful) { "create swarm failed: ${response.code}" }
                val json = Json.parseToJsonElement(response.body!!.string()).jsonObject
                return json.getValue("id").jsonPrimitive.content
            }
        }

        fun createCode(swarmId: String): String {
            val body = "{}".toRequestBody(jsonMediaType)
            http.newCall(authedRequest("$base/api/v1/swarms/$swarmId/codes").post(body).build()).execute().use { response ->
                check(response.isSuccessful) { "create code failed: ${response.code}" }
                val json = Json.parseToJsonElement(response.body!!.string()).jsonObject
                return json.getValue("code").jsonPrimitive.content
            }
        }
    }

    private fun registration(name: String, fingerprintByte: String) = DeviceRegistration(
        name = name,
        deviceType = DeviceType.CLIENT,
        machineId = "machine-$name",
        certFingerprint = fingerprintByte.repeat(32),
        platform = "test",
        appVersion = "0.1.0",
    )

    private suspend fun <T> ReceiveChannel<T>.expectOne(): T = withTimeout(5_000) { receive() }

    @Test
    fun `hello_ack reports a real session`() = runBlocking {
        val stunBase = startStunServer()
        val browser = Browser(stunBase, OkHttpClient())
        browser.loginFreshAccount("kt-signaling-hello-${Random.nextInt()}@example.test")
        val swarmId = browser.createSwarm("Home")
        val code = browser.createCode(swarmId)
        val registered = StunApiClient(stunBase).registerDevice(code, registration("dev", "aa"))

        val (client, _rx) = SignalingClient.connect(stunBase, registered.accessToken, registered.deviceId)
        assertTrue(client.sessionId.isNotEmpty())
        assertTrue(client.observedAddr.contains("127.0.0.1"))
        client.close()
    }

    @Test
    fun `presence fires on swarm mate connect`() = runBlocking {
        val stunBase = startStunServer()
        val browser = Browser(stunBase, OkHttpClient())
        browser.loginFreshAccount("kt-signaling-presence-${Random.nextInt()}@example.test")
        val swarmId = browser.createSwarm("Home")
        val api = StunApiClient(stunBase)
        val a = api.registerDevice(browser.createCode(swarmId), registration("a", "aa"))

        val (_aClient, aRx) = SignalingClient.connect(stunBase, a.accessToken, a.deviceId)
        val b = api.registerDevice(browser.createCode(swarmId), registration("b", "bb"))
        val (bClient, _bRx) = SignalingClient.connect(stunBase, b.accessToken, b.deviceId)

        when (val message = aRx.expectOne()) {
            is SignalMessage.Presence -> {
                assertEquals(b.deviceId, message.deviceId)
                assertTrue(message.online)
            }
            else -> throw AssertionError("expected Presence, got $message")
        }
        bClient.close()
    }

    @Test
    fun `signal relays between swarm mates with from stamped`() = runBlocking {
        val stunBase = startStunServer()
        val browser = Browser(stunBase, OkHttpClient())
        browser.loginFreshAccount("kt-signaling-relay-${Random.nextInt()}@example.test")
        val swarmId = browser.createSwarm("Home")
        val api = StunApiClient(stunBase)
        val a = api.registerDevice(browser.createCode(swarmId), registration("a", "aa"))
        val b = api.registerDevice(browser.createCode(swarmId), registration("b", "bb"))

        val (aClient, aRx) = SignalingClient.connect(stunBase, a.accessToken, a.deviceId)
        val (bClient, bRx) = SignalingClient.connect(stunBase, b.accessToken, b.deviceId)
        aRx.expectOne() // A's presence-of-B notification, not under test here

        val offer = SignalPayload.Offer(
            punchId = "p1",
            candidates = emptyList(),
            certFingerprint = "aa".repeat(32),
        )
        aClient.sendSignal(b.deviceId, offer)

        when (val message = bRx.expectOne()) {
            is SignalMessage.Signal -> {
                assertEquals(a.deviceId, message.from)
                assertEquals(b.deviceId, message.to)
                assertEquals(offer, message.payload)
            }
            else -> throw AssertionError("expected Signal, got $message")
        }
        aClient.close()
        bClient.close()
    }

    @Test
    fun `signal across swarms is rejected`() = runBlocking {
        val stunBase = startStunServer()
        val browser = Browser(stunBase, OkHttpClient())
        browser.loginFreshAccount("kt-signaling-cross-swarm-${Random.nextInt()}@example.test")
        val swarmA = browser.createSwarm("Home")
        val swarmB = browser.createSwarm("Cabin")
        val api = StunApiClient(stunBase)
        val a = api.registerDevice(browser.createCode(swarmA), registration("a", "aa"))
        val b = api.registerDevice(browser.createCode(swarmB), registration("b", "bb"))

        val (aClient, aRx) = SignalingClient.connect(stunBase, a.accessToken, a.deviceId)
        val (bClient, _bRx) = SignalingClient.connect(stunBase, b.accessToken, b.deviceId)

        aClient.sendSignal(b.deviceId, SignalPayload.Punched(punchId = "p1", ok = true))

        when (val message = aRx.expectOne()) {
            is SignalMessage.Error -> assertEquals("not_swarm_mates", message.code)
            else -> throw AssertionError("expected Error, got $message")
        }
        aClient.close()
        bClient.close()
    }

    @Test
    fun `a bad token is rejected at hello`() = runBlocking {
        val stunBase = startStunServer()
        val browser = Browser(stunBase, OkHttpClient())
        browser.loginFreshAccount("kt-signaling-bad-token-${Random.nextInt()}@example.test")
        val swarmId = browser.createSwarm("Home")
        val registered = StunApiClient(stunBase).registerDevice(browser.createCode(swarmId), registration("dev", "aa"))

        val error = assertThrows(SignalingError.Rejected::class.java) {
            runBlocking { SignalingClient.connect(stunBase, "not-the-real-token", registered.deviceId) }
        }
        assertEquals("unauthorized", error.code)
    }
}
