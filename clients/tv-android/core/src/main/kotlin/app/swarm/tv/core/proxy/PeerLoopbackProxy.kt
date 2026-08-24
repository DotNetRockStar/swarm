/**
 * Bridges standard HTTP (what Media3/ExoPlayer, or any off-the-shelf HTTP
 * client, speaks) to the SWARM peer protocol over QUIC — the loopback proxy
 * called for in `docs/PROTOCOL.md`'s connection-establishment notes and the
 * project plan's "how ExoPlayer plays HLS over a hole-punched QUIC
 * connection" design question. A media player is handed an ordinary
 * `http://127.0.0.1:<port>/<serverId>/<peerPath>` URL; this translates each
 * HTTP request (including `Range`) into a [PeerConnection.request] call
 * against the named server's live connection and streams the response back
 * with the right status/headers.
 *
 * One connection per registered server, keyed by an opaque `serverId` the
 * caller assigns (e.g. the device id from the swarm roster) — a swarm
 * typically has several servers, and the catalog merge logic
 * ([app.swarm.tv.core.catalog.CatalogMerger]) picks which one to stream an
 * asset from; this proxy just needs to know how to route to whichever one
 * was chosen.
 *
 * v1 is deliberately simple: one request per accepted socket
 * (`Connection: close`, no pipelining/keep-alive) and GET only — a local
 * loopback round-trip is sub-millisecond, so per-request connection setup
 * isn't a real cost, and every real client (ExoPlayer included) copes fine
 * with non-persistent HTTP/1.1 connections.
 */
package app.swarm.tv.core.proxy

import app.swarm.tv.core.peer.ByteRange
import app.swarm.tv.core.peer.PeerResponseHeader
import app.swarm.tv.core.transport.PeerConnection
import app.swarm.tv.core.transport.PeerQuicError
import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

private const val MAX_REQUEST_LINE_BYTES = 8 * 1024
private const val MAX_HEADER_BYTES = 32 * 1024
private const val COPY_BUFFER_BYTES = 64 * 1024

class PeerLoopbackProxy private constructor(
    private val serverSocket: ServerSocket,
    private val executor: ExecutorService,
) : AutoCloseable {
    private val connections = ConcurrentHashMap<String, PeerConnection>()

    val port: Int get() = serverSocket.localPort

    companion object {
        fun start(): PeerLoopbackProxy {
            // Bind the literal IPv4 loopback, not InetAddress.getLoopbackAddress()
            // — on this project's real Fire TV hardware that resolved to the
            // IPv6 loopback (::1), a socket urlFor()'s hardcoded "127.0.0.1"
            // URL can never reach, so every player request failed with
            // ConnectException despite the proxy being alive and listening.
            val serverSocket = ServerSocket(0, 50, InetAddress.getByName("127.0.0.1"))
            val executor = Executors.newCachedThreadPool { runnable ->
                Thread(runnable, "swarm-peer-proxy").apply { isDaemon = true }
            }
            val proxy = PeerLoopbackProxy(serverSocket, executor)
            val acceptThread = Thread(proxy::acceptLoop, "swarm-peer-proxy-accept")
            acceptThread.isDaemon = true
            acceptThread.start()
            return proxy
        }
    }

    /** Routes requests under `/<serverId>/...` to `connection` until [unregister]ed or [close]d. */
    fun register(serverId: String, connection: PeerConnection) {
        connections[serverId] = connection
    }

    /** Stops routing to `serverId`. Does not close the connection — the caller owns its lifecycle. */
    fun unregister(serverId: String) {
        connections.remove(serverId)
    }

    /** The URL to hand a media player for `peerPath` (e.g. `/media/<entryKey>`) on `serverId`. */
    fun urlFor(serverId: String, peerPath: String): String =
        "http://127.0.0.1:$port/$serverId$peerPath"

    override fun close() {
        runCatching { serverSocket.close() }
        executor.shutdownNow()
        connections.clear()
    }

    private fun acceptLoop() {
        while (!serverSocket.isClosed) {
            val socket = try {
                serverSocket.accept()
            } catch (e: SocketException) {
                break // socket closed under us — normal shutdown path
            } catch (e: IOException) {
                continue
            }
            executor.execute { handle(socket) }
        }
    }

    private fun handle(socket: Socket) {
        socket.use {
            socket.soTimeout = 10_000
            val input = BufferedInputStream(socket.getInputStream())
            val output = socket.getOutputStream()
            try {
                val requestLine = readLine(input, MAX_REQUEST_LINE_BYTES) ?: return
                val headers = readHeaders(input)
                serve(requestLine, headers, output)
            } catch (e: IOException) {
                // Peer went away mid-request (player seeked/aborted) — nothing to respond to.
            } catch (e: RuntimeException) {
                // Last-resort containment for transport implementations that
                // violate InputStream's IOException contract. An uncaught
                // exception on an Android executor thread is process-fatal.
            }
        }
    }

    private fun serve(requestLine: String, headers: Map<String, String>, output: OutputStream) {
        val parts = requestLine.split(' ')
        if (parts.size < 2 || parts[0] != "GET") {
            writeStatusOnly(output, 405, "Method Not Allowed")
            return
        }
        val requestTarget = parts[1]
        val fullPath = requestTarget.substringBefore('?')
        val query = requestTarget.substringAfter('?', missingDelimiterValue = "").takeIf { it.isNotEmpty() }
        val firstSlash = fullPath.indexOf('/', 1)
        val serverId: String
        val peerPath: String
        if (firstSlash < 0) {
            serverId = fullPath.removePrefix("/")
            peerPath = "/" + query?.let { "?$it" }.orEmpty()
        } else {
            serverId = fullPath.substring(1, firstSlash)
            peerPath = fullPath.substring(firstSlash) + query?.let { "?$it" }.orEmpty()
        }
        val connection = connections[serverId]
        if (connection == null) {
            writeStatusOnly(output, 404, "Not Found")
            return
        }

        val range = headers["range"]?.let(::parseRangeHeader)
        val ifNoneMatch = headers["if-none-match"]?.trim('"')

        val response = try {
            connection.request(peerPath, range, ifNoneMatch)
        } catch (e: PeerQuicError) {
            writeStatusOnly(output, 503, "Service Unavailable")
            return
        } catch (e: IOException) {
            // Real bug, found live: this used to remove `serverId` from
            // `connections` right here on any single failed request. Fine
            // for one deliberate action (the caller notices a play/browse
            // attempt failed and retries), but silently fatal for artwork —
            // a burst of concurrent card-image fetches sharing one
            // connection (a normal grid scroll) meant one transient hiccup
            // on any single request deregistered the connection for the
            // *whole server*, and every image fetch after that point 404'd
            // instantly with no attempt to reconnect, until whatever next
            // triggered a full catalog refresh. This proxy has no business
            // making that call anyway — see the class doc comment
            // ("this proxy just needs to know how to route to whichever one
            // was chosen" / "the caller owns its lifecycle"). Connection
            // health and reconnection are entirely
            // [app.swarm.tv.core.catalog.CatalogSession]'s job now, via the
            // self-healing [PeerConnection] it registers.
            writeStatusOnly(output, 503, "Service Unavailable")
            return
        } catch (e: RuntimeException) {
            // A closed kwik connection can race a new stream request and
            // throw unchecked. Keep that failure inside the proxy boundary
            // so Media3 receives a recoverable server-offline response.
            writeStatusOnly(output, 503, "Service Unavailable")
            return
        }

        // Media3 abandons the current HTTP request whenever the user seeks
        // and immediately opens a replacement Range request. Closing the
        // peer body in both the success and aborted-socket paths propagates
        // that cancellation to the old QUIC stream instead of leaving the
        // server uploading an obsolete range in the background.
        response.body.use { body -> writeResponse(output, response.header, body) }
    }

    private fun writeResponse(output: OutputStream, header: PeerResponseHeader, body: InputStream) {
        val builder = StringBuilder()
        builder.append("HTTP/1.1 ").append(header.status).append(' ').append(reasonPhrase(header.status)).append("\r\n")
        builder.append("Content-Length: ").append(header.len).append("\r\n")
        builder.append("Content-Type: ").append(header.contentType ?: "application/octet-stream").append("\r\n")
        builder.append("Accept-Ranges: bytes\r\n")
        header.contentRange?.let {
            builder.append("Content-Range: bytes ").append(it.start).append('-').append(it.end).append('/').append(it.total).append("\r\n")
        }
        header.etag?.let { builder.append("ETag: \"").append(it).append("\"\r\n") }
        builder.append("Connection: close\r\n\r\n")
        output.write(builder.toString().toByteArray(Charsets.US_ASCII))

        val buffer = ByteArray(COPY_BUFFER_BYTES)
        while (true) {
            val read = body.read(buffer)
            if (read < 0) break
            output.write(buffer, 0, read)
        }
        output.flush()
    }

    private fun writeStatusOnly(output: OutputStream, status: Int, reason: String) {
        val text = "HTTP/1.1 $status $reason\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        output.write(text.toByteArray(Charsets.US_ASCII))
        output.flush()
    }

    private fun readLine(input: InputStream, limit: Int): String? {
        val buffer = ByteArrayOutputStream()
        var previousWasCr = false
        while (true) {
            val b = input.read()
            if (b < 0) return if (buffer.size() == 0) null else buffer.toString(Charsets.US_ASCII.name())
            if (b == '\n'.code) {
                val bytes = buffer.toByteArray()
                val end = if (previousWasCr && bytes.isNotEmpty()) bytes.size - 1 else bytes.size
                return String(bytes, 0, end, Charsets.US_ASCII)
            }
            previousWasCr = b == '\r'.code
            buffer.write(b)
            if (buffer.size() > limit) throw IOException("request line exceeded $limit bytes")
        }
    }

    private fun readHeaders(input: InputStream): Map<String, String> {
        val headers = mutableMapOf<String, String>()
        var totalBytes = 0
        while (true) {
            val line = readLine(input, MAX_HEADER_BYTES) ?: break
            totalBytes += line.length
            if (totalBytes > MAX_HEADER_BYTES) throw IOException("headers exceeded $MAX_HEADER_BYTES bytes")
            if (line.isEmpty()) break
            val colon = line.indexOf(':')
            if (colon <= 0) continue
            headers[line.substring(0, colon).trim().lowercase()] = line.substring(colon + 1).trim()
        }
        return headers
    }
}

/**
 * `Range: bytes=N-M` / `bytes=N-` / `bytes=-N`. Only the first range in a
 * comma-separated list is honored — the SWARM peer protocol has no
 * multipart/byteranges concept, matching `swarm-media`'s server-side parser.
 */
internal fun parseRangeHeader(value: String): ByteRange? {
    val spec = value.removePrefix("bytes=").substringBefore(',').trim()
    val dash = spec.indexOf('-')
    if (dash < 0) return null
    val startPart = spec.substring(0, dash)
    val endPart = spec.substring(dash + 1)
    return when {
        startPart.isEmpty() -> endPart.toLongOrNull()?.takeIf { it > 0 }?.let { ByteRange.Suffix(it) }
        else -> startPart.toLongOrNull()?.let { start -> ByteRange.FromTo(start, endPart.toLongOrNull()) }
    }
}

private fun reasonPhrase(status: Int): String = when (status) {
    200 -> "OK"
    206 -> "Partial Content"
    304 -> "Not Modified"
    400 -> "Bad Request"
    404 -> "Not Found"
    429 -> "Too Many Requests"
    416 -> "Range Not Satisfiable"
    503 -> "Service Unavailable"
    else -> "Internal Server Error"
}
