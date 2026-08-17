/**
 * The capstone of the Kotlin hole-punch port: a real Kotlin client
 * negotiates and completes a hole-punched QUIC connection against a real
 * Rust `swarm-serverd` — auto-registered via `SWARM_STUN_CODE` exactly like
 * a headless deployment would, answering the offer entirely on its own via
 * `ServerCore`'s punch-dispatch loop (`apps/server/src/lib.rs`). No test
 * code on the Rust side; `initiatePunchConnection` is the only thing
 * driving this from the Kotlin side. Every primitive underneath (signaling,
 * reflector, punch, the kwik socket-continuity trick) already has its own
 * focused proof — this is the thing none of those can show alone, that the
 * wiring is correct end to end across both languages' full stacks.
 */
package app.swarm.tv.core.transport

import app.swarm.tv.core.client.SignalingClient
import app.swarm.tv.core.client.StunApiClient
import app.swarm.tv.core.rest.DeviceRegistration
import app.swarm.tv.core.rest.DeviceType
import java.io.BufferedReader
import java.io.File
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.util.concurrent.TimeUnit
import java.util.regex.Pattern
import kotlin.random.Random
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.junit.jupiter.api.AfterEach
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assumptions.assumeTrue
import org.junit.jupiter.api.Test

private val jsonMediaType = "application/json".toMediaType()

class PunchConnectInteropTest {
    private val processes = mutableListOf<Process>()
    private val tempDirs = mutableListOf<File>()

    @AfterEach
    fun tearDown() {
        processes.forEach { it.destroyForcibly().waitFor(5, TimeUnit.SECONDS) }
        tempDirs.forEach { it.deleteRecursively() }
    }

    private fun stunServerBinary(): File? =
        File("../../../target/release/swarm-stun-server").canonicalFile.takeIf { it.exists() }

    private fun mediaServerBinary(): File? =
        File("../../../target/release/swarm-serverd").canonicalFile.takeIf { it.exists() }

    private fun freeUdpPort(): Int {
        DatagramSocket(0).use { return it.localPort }
    }

    private fun stripAnsi(line: String) = line.replace(Regex("\\[[0-9;]*m"), "")

    private fun drain(proc: Process, tag: String) {
        val reader = BufferedReader(proc.inputStream.reader())
        Thread({ generateSequence { reader.readLine() }.forEach { println("[$tag] $it") } }, "$tag-drain")
            .apply { isDaemon = true }.start()
    }

    private fun startStunServer(reflectorPort: Int): String {
        val binary = stunServerBinary()!!
        val dbPath = File.createTempFile("swarm-punchconnect-stun", ".sqlite").apply { deleteOnExit() }
        val builder = ProcessBuilder(binary.absolutePath).redirectErrorStream(true)
        builder.environment().apply {
            put("SWARM_DATABASE_PATH", dbPath.absolutePath)
            put("SWARM_HTTP_BIND", "127.0.0.1:0")
            put("SWARM_REFLECTOR_PORTS", reflectorPort.toString())
            put("RUST_LOG", "info")
        }
        val proc = builder.start()
        processes += proc

        val reader = BufferedReader(proc.inputStream.reader())
        val bindPattern = Pattern.compile("bind=(\\d+\\.\\d+\\.\\d+\\.\\d+:\\d+)")
        var bind: String? = null
        val deadline = System.currentTimeMillis() + 15_000
        while (System.currentTimeMillis() < deadline && bind == null) {
            val line = stripAnsi(reader.readLine() ?: break)
            bindPattern.matcher(line).takeIf { it.find() }?.let { bind = it.group(1) }
        }
        checkNotNull(bind) { "swarm-stun-server did not report readiness within the deadline" }
        drain(proc, "swarm-stun-server")
        return "http://$bind"
    }

    /** Returns the server's self-reported cert fingerprint once it's confirmed it joined the swarm. */
    private fun startMediaServer(stunBase: String, code: String): String {
        val binary = mediaServerBinary()!!
        val mediaRoot = File.createTempFile("swarm-punchconnect-media", "").apply { delete(); mkdirs() }
        val dataDir = File.createTempFile("swarm-punchconnect-data", "").apply { delete(); mkdirs() }
        tempDirs += mediaRoot
        tempDirs += dataDir
        val builder = ProcessBuilder(binary.absolutePath).redirectErrorStream(true)
        builder.environment().apply {
            put("SWARM_MEDIA_ROOT", mediaRoot.absolutePath)
            put("SWARM_DATA_DIR", dataDir.absolutePath)
            put("SWARM_PEER_BIND", "127.0.0.1:0")
            put("SWARM_STUN_URL", stunBase)
            put("SWARM_STUN_CODE", code)
            put("SWARM_TOKEN_STORE_FILE_ONLY", "1")
            put("RUST_LOG", "info")
        }
        val proc = builder.start()
        processes += proc

        val reader = BufferedReader(proc.inputStream.reader())
        val fingerprintPattern = Pattern.compile("fingerprint=([0-9a-f]{64})")
        val joinedPattern = Pattern.compile("joined swarm via SWARM_STUN_CODE")
        var fingerprint: String? = null
        var joined = false
        val deadline = System.currentTimeMillis() + 15_000
        while (System.currentTimeMillis() < deadline && (fingerprint == null || !joined)) {
            val line = stripAnsi(reader.readLine() ?: break)
            fingerprintPattern.matcher(line).takeIf { it.find() }?.let { fingerprint = it.group(1) }
            if (joinedPattern.matcher(line).find()) joined = true
        }
        checkNotNull(fingerprint) { "swarm-serverd did not report its identity fingerprint within the deadline" }
        check(joined) { "swarm-serverd did not confirm joining the swarm within the deadline" }
        drain(proc, "swarm-serverd")
        return fingerprint!!
    }

    /** Minimal cookie-session client for the STUN web API. */
    private class Browser(private val base: String, private val http: OkHttpClient) {
        private lateinit var cookieHeader: String
        private lateinit var csrf: String

        fun loginFreshAccount(email: String) {
            val password = "correct horse battery"
            val body = """{"email":"$email","password":"$password"}""".toRequestBody(jsonMediaType)
            http.newCall(Request.Builder().url("$base/api/v1/auth/register").post(body).build()).execute().use {
                check(it.isSuccessful) { "register failed: ${it.code}" }
            }
            http.newCall(Request.Builder().url("$base/api/v1/auth/login").post(body).build()).execute().use { response ->
                check(response.isSuccessful) { "login failed: ${response.code}" }
                var session = ""
                var csrfValue = ""
                for (header in response.headers("set-cookie")) {
                    val (name, value) = header.substringBefore(';').split('=', limit = 2)
                    when (name) {
                        "swarm_session" -> session = value
                        "swarm_csrf" -> csrfValue = value
                    }
                }
                cookieHeader = "swarm_session=$session; swarm_csrf=$csrfValue"
                csrf = csrfValue
            }
        }

        private fun authed(url: String) = Request.Builder().url(url).header("cookie", cookieHeader).header("x-swarm-csrf", csrf)

        fun createSwarm(name: String): String {
            val body = """{"name":"$name"}""".toRequestBody(jsonMediaType)
            http.newCall(authed("$base/api/v1/swarms").post(body).build()).execute().use { response ->
                check(response.isSuccessful) { "create swarm failed: ${response.code}" }
                return Json.parseToJsonElement(response.body!!.string()).jsonObject.getValue("id").jsonPrimitive.content
            }
        }

        fun createCode(swarmId: String): String {
            val body = "{}".toRequestBody(jsonMediaType)
            http.newCall(authed("$base/api/v1/swarms/$swarmId/codes").post(body).build()).execute().use { response ->
                check(response.isSuccessful) { "create code failed: ${response.code}" }
                return Json.parseToJsonElement(response.body!!.string()).jsonObject.getValue("code").jsonPrimitive.content
            }
        }

        /** Finds the roster device id whose `cert_fingerprint` matches [fingerprint]. */
        fun deviceIdFor(swarmId: String, fingerprint: String): String {
            http.newCall(authed("$base/api/v1/swarms/$swarmId/devices").build()).execute().use { response ->
                check(response.isSuccessful) { "list devices failed: ${response.code}" }
                val devices = Json.parseToJsonElement(response.body!!.string()).jsonObject.getValue("devices").jsonArray
                val match = devices.first { it.jsonObject.getValue("cert_fingerprint").jsonPrimitive.content == fingerprint }
                return match.jsonObject.getValue("device_id").jsonPrimitive.content
            }
        }
    }

    @Test
    fun `a real kotlin client punches to a real rust server with no test code on the rust side`() {
        assumeTrue(stunServerBinary() != null, "swarm-stun-server release binary not found")
        assumeTrue(mediaServerBinary() != null, "swarm-serverd release binary not found")

        runBlocking {
            val reflectorPort = freeUdpPort()
            val stunBase = startStunServer(reflectorPort)
            val browser = Browser(stunBase, OkHttpClient())
            browser.loginFreshAccount("kt-punch-connect-${Random.nextInt()}@example.test")
            val swarmId = browser.createSwarm("Home")

            // The client registers *before* the server starts: ServerCore's
            // roster sync only runs once synchronously during
            // register_with_stun and then every 30s after — with no handle
            // to that separate process to force an early resync (unlike
            // the same-process Rust tests, which call .resync() directly),
            // the client has to already be in the swarm roster by the time
            // the server's one guaranteed sync happens, or its
            // RosterClientVerifier won't have the client's fingerprint yet
            // and the QUIC handshake will be refused on first use.
            val clientIdentity = TestIdentity.generate("punch-connect-client")
            val clientCode = browser.createCode(swarmId)
            val clientReg = StunApiClient(stunBase).registerDevice(
                clientCode,
                DeviceRegistration(
                    name = "client",
                    deviceType = DeviceType.CLIENT,
                    machineId = "machine-client",
                    certFingerprint = clientIdentity.fingerprint,
                    platform = "test",
                    appVersion = "0.1.0",
                ),
            )
            val (signaling, signalRx) = SignalingClient.connect(stunBase, clientReg.accessToken, clientReg.deviceId)
            val reflectorAddr = InetSocketAddress("127.0.0.1", reflectorPort)

            val serverCode = browser.createCode(swarmId)
            val serverFingerprint = startMediaServer(stunBase, serverCode)
            val serverDeviceId = browser.deviceIdFor(swarmId, serverFingerprint)

            val peer = initiatePunchConnection(
                signaling = signaling,
                signalRx = signalRx,
                reflectorAddr = reflectorAddr,
                peerDeviceId = serverDeviceId,
                ownFingerprint = clientIdentity.fingerprint,
                clientCertificate = clientIdentity.certificate,
                clientKey = clientIdentity.privateKey,
                expectedFingerprint = serverFingerprint,
            )
            peer.use {
                val response = it.request("/catalog/thumbprint")
                assertEquals(200, response.header.status)
            }
            signaling.close()
        }
    }
}
