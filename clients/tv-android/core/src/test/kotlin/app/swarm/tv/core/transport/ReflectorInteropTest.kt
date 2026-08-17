/**
 * Proves `reflexiveAddr` against the real reflector implementation
 * (`stun_server::reflector::run`, reached by spawning the release
 * `swarm-stun-server` binary with a real reflector port configured) — the
 * Kotlin counterpart to `crates/swarm-p2p/src/reflector.rs`'s own tests.
 * Skips (not fails) if the binary hasn't been built, same discipline as
 * every other interop test in this module.
 */
package app.swarm.tv.core.transport

import java.io.BufferedReader
import java.io.File
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.util.concurrent.TimeUnit
import java.util.regex.Pattern
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.AfterEach
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Assumptions.assumeTrue
import org.junit.jupiter.api.BeforeEach
import org.junit.jupiter.api.Test

class ReflectorInteropTest {
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

    /** An ephemeral port picked by binding then immediately freeing a socket — the same probe-then-drop technique used throughout this project's Rust tests. */
    private fun freeUdpPort(): Int {
        DatagramSocket(0).use { return it.localPort }
    }

    private fun startStunServerWithReflector(reflectorPort: Int): Process {
        val binary = resolveServerBinary()!!
        val dbPath = File.createTempFile("swarm-reflector-interop", ".sqlite").apply { deleteOnExit() }
        val builder = ProcessBuilder(binary.absolutePath).redirectErrorStream(true)
        builder.environment().apply {
            put("SWARM_DATABASE_PATH", dbPath.absolutePath)
            put("SWARM_HTTP_BIND", "127.0.0.1:0")
            put("SWARM_REFLECTOR_PORTS", reflectorPort.toString())
            put("RUST_LOG", "info")
        }
        val proc = builder.start()
        process = proc

        // The reflector task spawns before the HTTP listener binds+logs, so
        // waiting for the "bind=" line guarantees the reflector is already up.
        val reader = BufferedReader(proc.inputStream.reader())
        val bindPattern = Pattern.compile("bind=(\\d+\\.\\d+\\.\\d+\\.\\d+:\\d+)")
        var ready = false
        val deadline = System.currentTimeMillis() + 15_000
        while (System.currentTimeMillis() < deadline && !ready) {
            val rawLine = reader.readLine() ?: break
            val line = rawLine.replace(Regex("\\[[0-9;]*m"), "")
            if (bindPattern.matcher(line).find()) ready = true
        }
        check(ready) { "swarm-stun-server did not report readiness within the deadline" }
        Thread({
            generateSequence { reader.readLine() }.forEach { println("[swarm-stun-server] $it") }
        }, "swarm-stun-server-drain").apply { isDaemon = true }.start()
        return proc
    }

    @Test
    fun `learns its own observed address from the real reflector`() = runBlocking {
        val reflectorPort = freeUdpPort()
        startStunServerWithReflector(reflectorPort)
        val reflectorAddr = InetSocketAddress("127.0.0.1", reflectorPort)

        DatagramSocket(0).use { socket ->
            val observed = reflexiveAddr(socket, reflectorAddr)
            assertEquals("127.0.0.1", observed.address.hostAddress)
            assertEquals(socket.localPort, observed.port)
        }
    }

    @Test
    fun `nothing listening is a timeout`() = runBlocking {
        // No server spawned at all — port 1 is privileged and unbound in any test env.
        val nobody = InetSocketAddress("127.0.0.1", 1)
        DatagramSocket(0).use { socket ->
            assertThrows(ReflectorError.Timeout::class.java) {
                runBlocking { reflexiveAddr(socket, nobody) }
            }
        }
    }
}
