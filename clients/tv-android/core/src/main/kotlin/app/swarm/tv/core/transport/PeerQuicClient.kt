/**
 * Device <-> device connection over QUIC, mirroring
 * `swarm_p2p::endpoint::connect` + `send_request` (Rust) on top of kwik (a
 * pure-JVM QUIC implementation — the only realistic option for Android,
 * where a native quinn/rustls binding isn't practical).
 *
 * **Trust model differs mechanically from the Rust side, same guarantee.**
 * Rust pins the server by installing a custom `ServerCertVerifier` that
 * compares the presented certificate's SHA-256 against the expected
 * fingerprint *during* the TLS handshake. kwik's public `Builder` only
 * exposes `noServerCertificateCheck()` or a CA-style `customTrustStore`
 * (checked against the exploration of `tech.kwik:kwik:0.10.3`'s actual
 * class file — no certificate-verification callback exists) — pinning a
 * single self-signed leaf certificate with no CA doesn't fit that shape.
 * So this client disables kwik's own check and verifies the fingerprint
 * itself immediately after the handshake, via
 * `QuicClientConnection.getServerCertificateChain()`, closing the
 * connection on any mismatch before a single request is sent. No
 * application data leaves the client on an unpinned connection either way;
 * the security property (never send anything to a server whose certificate
 * doesn't match the pin) is the same, just enforced one layer up.
 *
 * The client certificate side has no such gap: kwik's builder takes an
 * `X509Certificate` + `PrivateKey` directly
 * (`clientCertificate`/`clientCertificateKey`), which is exactly what the
 * server's `RosterClientVerifier` (Rust) requires for the mTLS handshake.
 */
package app.swarm.tv.core.transport

import app.swarm.tv.core.peer.ByteRange
import app.swarm.tv.core.peer.ClientErrorReport
import app.swarm.tv.core.peer.LikeToggle
import app.swarm.tv.core.peer.PeerRequest
import app.swarm.tv.core.peer.PeerResponseHeader
import app.swarm.tv.core.peer.PlaybackPreferences
import app.swarm.tv.core.rest.SwarmJson
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.net.DatagramSocket
import java.net.InetAddress
import java.security.MessageDigest
import java.security.PrivateKey
import java.security.cert.X509Certificate
import java.time.Duration
import java.util.concurrent.Callable
import java.util.concurrent.ExecutionException
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import kotlinx.serialization.decodeFromString
import kotlinx.serialization.encodeToString
import tech.kwik.core.DatagramSocketFactory
import tech.kwik.core.QuicClientConnection
import tech.kwik.core.QuicStream
import tech.kwik.core.log.Logger
import tech.kwik.core.log.NullLogger

/** ALPN identifying the SWARM peer protocol; bump alongside the wire format. */
const val PEER_ALPN: String = "swarm-peer/1"

/**
 * How long [PeerQuicClient.request] waits for a new bidirectional QUIC
 * stream before giving up on the connection.
 *
 * kwik's `QuicConnection.createStream(true)` *blocks* — for up to 10,000
 * days, its hard-coded internal default — whenever the peer has not issued
 * enough stream-count credit. quinn (the Rust server) hands out that credit
 * by advancing MAX_STREAMS as its own streams finish; if one server-side
 * stream stalls (seen live at the very end of a long movie — issue #140),
 * MAX_STREAMS stops advancing, the client's window is exhausted, and every
 * subsequent request on that connection — catalog refresh, playback
 * negotiation, artwork — parks in `createStream` forever. Closing the
 * connection does **not** release that wait (`StreamManager.abortAll()`
 * only touches already-created streams), so the whole app wedges with no
 * path to recovery except a brand-new connection, which is exactly what a
 * manual "Browse All" library reload happened to force.
 *
 * Bounding stream creation turns that indefinite hang into an ordinary
 * `IOException` that [app.swarm.tv.core.catalog.CatalogSession]'s existing
 * evict-and-reconnect logic already recovers from automatically. Kept below
 * `CatalogSession.MANIFEST_FETCH_TIMEOUT_MS` so a starved connection fails
 * its request outright rather than first tripping the outer per-attempt
 * race; comfortably above any wait a healthy connection momentarily at its
 * concurrency limit could impose.
 */
internal const val STREAM_CREATE_TIMEOUT_MS: Long = 10_000L

/** Shared daemon pool for the [openBoundedStream] watchdog — threads are
 * only borrowed for the (near-instant, on a healthy connection) duration of
 * one `createStream` call and reclaimed after 60s idle. */
private val streamCreateExecutor: ExecutorService = Executors.newCachedThreadPool { runnable ->
    Thread(runnable, "swarm-peer-createstream").apply { isDaemon = true }
}

/**
 * Runs [create] (a blocking `createStream` call) on a worker thread and
 * abandons it after [timeoutMs], interrupting the blocked call — kwik's
 * stream-credit wait is interruptible and unwinds to a `TimeoutException`
 * internally — so a peer that has stopped issuing stream credit surfaces as
 * a transport failure instead of an unbounded hang. See
 * [STREAM_CREATE_TIMEOUT_MS].
 */
@Throws(IOException::class)
internal fun openBoundedStream(
    create: () -> QuicStream,
    timeoutMs: Long = STREAM_CREATE_TIMEOUT_MS,
    executor: ExecutorService = streamCreateExecutor,
): QuicStream {
    val task = executor.submit(Callable { create() })
    return try {
        task.get(timeoutMs, TimeUnit.MILLISECONDS)
    } catch (timeout: TimeoutException) {
        task.cancel(true)
        throw IOException(
            "peer stream creation stalled; aborted after ${timeoutMs}ms (peer issued no stream credit)",
        )
    } catch (interrupted: InterruptedException) {
        task.cancel(true)
        Thread.currentThread().interrupt()
        throw IOException("peer stream creation interrupted", interrupted)
    } catch (failed: ExecutionException) {
        when (val cause = failed.cause) {
            is IOException -> throw cause
            null -> throw IOException("peer stream creation failed")
            else -> throw IOException(
                "peer stream creation failed: ${cause.message ?: cause.javaClass.simpleName}",
                cause,
            )
        }
    }
}

/** Irrelevant under pinning (no hostname check happens), kept only because kwik requires *a* host. */
private const val SNI_PLACEHOLDER = "swarm-peer"
private const val MAX_HEADER_LINE_BYTES = 64 * 1024

sealed class PeerQuicError(message: String) : IOException(message) {
    data object NoServerCertificate : PeerQuicError("server presented no certificate")
    data class FingerprintMismatch(val expected: String, val actual: String) :
        PeerQuicError("server certificate fingerprint mismatch: expected $expected, got $actual")
    data object TruncatedHeader : PeerQuicError("connection closed before a full response header line was read")
    data object HeaderTooLong : PeerQuicError("response header line exceeded $MAX_HEADER_LINE_BYTES bytes")
    data class TruncatedBody(val gotBytes: Long, val expectedBytes: Long) :
        PeerQuicError("body truncated at $gotBytes/$expectedBytes bytes")
}

/** A response header plus a stream bounded to exactly `header.len` bytes. */
class PeerResponse(val header: PeerResponseHeader, val body: InputStream)

/**
 * What [app.swarm.tv.core.proxy.PeerLoopbackProxy] (and anything else that
 * just wants to issue peer requests) depends on, rather than the concrete
 * kwik-backed [PeerQuicClient] directly — lets that HTTP-translation layer
 * be tested against a fake, with no live QUIC connection required.
 */
interface PeerConnection {
    fun request(
        path: String,
        range: ByteRange? = null,
        ifNoneMatch: String? = null,
        playback: PlaybackPreferences? = null,
        errorReport: ClientErrorReport? = null,
        like: LikeToggle? = null,
    ): PeerResponse
}

class PeerQuicClient private constructor(private val connection: QuicClientConnection) : PeerConnection, AutoCloseable {

    companion object {
        /**
         * Opens a QUIC connection, presenting `clientCertificate`/`clientKey`
         * for mTLS, and verifies the server's certificate matches
         * `expectedServerFingerprint` (lowercase hex SHA-256) before
         * returning. Throws [PeerQuicError.FingerprintMismatch] — closing
         * the connection first — on any pin failure.
         *
         * [localSocketPort], when given, pins the connection's local UDP
         * port — the hole-punch continuity trick: unlike quinn (Rust side),
         * which takes literal ownership of an already-bound socket
         * (`Endpoint::new`), kwik's only hook is
         * `DatagramSocketFactory.createSocket`, which *creates a new*
         * socket rather than accepting an existing one. Binding that new
         * socket to the exact port a prior raw punch socket used — closed
         * immediately before this call — is the closest equivalent
         * available: it relies on the NAT mapping surviving that gap,
         * true for typical endpoint-independent-mapping NATs, not
         * guaranteed for every NAT type. See [PunchedSocketFactory].
         */
        @Throws(IOException::class)
        fun connect(
            host: String,
            port: Int,
            clientCertificate: X509Certificate,
            clientKey: PrivateKey,
            expectedServerFingerprint: String,
            connectTimeout: Duration = Duration.ofSeconds(5),
            logger: Logger = NullLogger(),
            localSocketPort: Int? = null,
        ): PeerQuicClient {
            val builder = QuicClientConnection.newBuilder()
                .host(host)
                .port(port)
                .applicationProtocol(PEER_ALPN)
                .connectTimeout(connectTimeout)
                .clientCertificate(clientCertificate)
                .clientCertificateKey(clientKey)
                .noServerCertificateCheck() // we verify by fingerprint below, not by CA chain
                .logger(logger)
            if (localSocketPort != null) {
                builder.socketFactory(PunchedSocketFactory(localSocketPort))
            }
            val connection = builder.build()
            connection.connect()

            val presented = connection.serverCertificateChain.firstOrNull()
            if (presented == null) {
                connection.close()
                throw PeerQuicError.NoServerCertificate
            }
            val actual = sha256Hex(presented.encoded)
            if (!actual.equals(expectedServerFingerprint, ignoreCase = true)) {
                connection.close()
                throw PeerQuicError.FingerprintMismatch(expected = expectedServerFingerprint, actual = actual)
            }
            return PeerQuicClient(connection)
        }

        private fun sha256Hex(bytes: ByteArray): String =
            MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
    }

    /**
     * One request, one fresh bidirectional QUIC stream — matches the Rust
     * side's "one request per stream" model (no head-of-line blocking
     * across concurrent requests on the same connection).
     */
    @Throws(IOException::class)
    override fun request(
        path: String,
        range: ByteRange?,
        ifNoneMatch: String?,
        playback: PlaybackPreferences?,
        errorReport: ClientErrorReport?,
        like: LikeToggle?,
    ): PeerResponse {
        return try {
            val stream: QuicStream = openBoundedStream(create = { connection.createStream(true) })
            val requestLine = SwarmJson.encodeToString(PeerRequest(path, range, ifNoneMatch, playback, errorReport, like)) + "\n"
            stream.outputStream.use { it.write(requestLine.toByteArray(Charsets.UTF_8)) }

            val input = stream.inputStream
            val header = SwarmJson.decodeFromString<PeerResponseHeader>(readHeaderLine(input))
            PeerResponse(header, BoundedInputStream(input, header.len))
        } catch (error: IOException) {
            throw error
        } catch (error: RuntimeException) {
            // kwik reports some connection-closed races as unchecked
            // exceptions (for example while creating a stream). Let every
            // caller handle those as an ordinary transport failure instead
            // of allowing an executor thread's uncaught exception to bring
            // down the Android process.
            throw IOException(
                "peer request failed: ${error.message ?: error.javaClass.simpleName}",
                error,
            )
        }
    }

    override fun close() {
        connection.close()
    }

    private fun readHeaderLine(input: InputStream): String {
        val buffer = ByteArrayOutputStream()
        while (true) {
            val b = input.read()
            if (b < 0) throw PeerQuicError.TruncatedHeader
            if (b == '\n'.code) break
            buffer.write(b)
            if (buffer.size() > MAX_HEADER_LINE_BYTES) throw PeerQuicError.HeaderTooLong
        }
        return buffer.toString(Charsets.UTF_8.name())
    }
}

/**
 * Binds a fresh [DatagramSocket] to [port] every time kwik asks for one —
 * see [PeerQuicClient.connect]'s doc comment for why "fresh socket, same
 * port" is the closest thing to quinn's real socket handoff that kwik's
 * `DatagramSocketFactory` hook allows.
 */
private class PunchedSocketFactory(private val port: Int) : DatagramSocketFactory {
    override fun createSocket(destination: InetAddress): DatagramSocket = DatagramSocket(port)
}

/** Reads at most `limit` bytes from `delegate`, then always reports EOF. */
internal class BoundedInputStream(private val delegate: InputStream, private val limit: Long) : InputStream() {
    private var remaining = limit

    private fun interrupted(error: IOException): IOException = IOException(
        "response body interrupted at ${limit - remaining}/$limit bytes: ${error.message ?: error.javaClass.simpleName}",
        error,
    )

    private fun interrupted(error: RuntimeException): IOException = IOException(
        "response body interrupted at ${limit - remaining}/$limit bytes: ${error.message ?: error.javaClass.simpleName}",
        error,
    )

    override fun read(): Int {
        if (remaining <= 0) return -1
        val b = try {
            delegate.read()
        } catch (error: IOException) {
            throw interrupted(error)
        } catch (error: RuntimeException) {
            throw interrupted(error)
        }
        if (b < 0) throw PeerQuicError.TruncatedBody(limit - remaining, limit)
        if (b >= 0) remaining--
        return b
    }

    override fun read(b: ByteArray, off: Int, len: Int): Int {
        if (remaining <= 0) return -1
        val toRead = minOf(len.toLong(), remaining).toInt()
        val got = try {
            delegate.read(b, off, toRead)
        } catch (error: IOException) {
            throw interrupted(error)
        } catch (error: RuntimeException) {
            throw interrupted(error)
        }
        if (got < 0) throw PeerQuicError.TruncatedBody(limit - remaining, limit)
        if (got > 0) remaining -= got
        return got
    }

    override fun close() {
        delegate.close()
    }
}
