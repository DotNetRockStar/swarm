/**
 * Device-side WSS signaling client, mirroring
 * `swarm-stun-client::signaling::SignalingClient` (Rust): the persistent
 * connection a device holds open to receive presence updates and relay
 * hole-punch negotiation (`signal`) with its swarm-mates. Built on OkHttp's
 * `WebSocket` (already a dependency via [StunApiClient]) rather than a new
 * library, same reasoning as reusing OkHttp for Coil's network engine.
 */
package app.swarm.tv.core.client

import app.swarm.tv.core.capability.CapabilityProfile
import app.swarm.tv.core.rest.SwarmJson
import app.swarm.tv.core.signal.PROTOCOL_VERSION
import app.swarm.tv.core.signal.SignalMessage
import app.swarm.tv.core.signal.SignalPayload
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.decodeFromString
import kotlinx.serialization.encodeToString
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener

private const val PING_INTERVAL_MS = 30_000L
private const val HELLO_ACK_TIMEOUT_MS = 10_000L

sealed class SignalingError(message: String) : Exception(message) {
    class InvalidBaseUrl(baseUrl: String) :
        SignalingError("SWARM server URL must start with http:// or https://, got: $baseUrl")
    class Connect(cause: Throwable) : SignalingError("could not connect to the signaling endpoint: ${cause.message}")
    data object HelloTimeout : SignalingError("timed out waiting for hello_ack")
    data object ConnectionClosed : SignalingError("connection closed before hello_ack")
    class Rejected(val code: String, val serverMessage: String) :
        SignalingError("server rejected hello ($code): $serverMessage")
    data object Closed : SignalingError("signaling connection is closed")
}

/**
 * A live signaling session. [connect] hands back both this (the send side)
 * and a [ReceiveChannel] carrying every subsequent `presence`, `signal`, and
 * `error` message — `hello`/`hello_ack`/`ping`/`pong` are consumed
 * internally and never forwarded, so a caller's receive loop only ever sees
 * messages worth acting on. Mirrors the Rust client's
 * `(SignalingClient, mpsc::UnboundedReceiver<SignalMessage>)` return shape.
 */
class SignalingClient private constructor(
    val sessionId: String,
    val observedAddr: String,
    val reflectorPorts: List<Int>,
    private val webSocket: WebSocket,
    private val scope: CoroutineScope,
) {
    companion object {
        private val defaultClient = OkHttpClient.Builder()
            // A WebSocket is long-lived and legitimately idle between our
            // own PING_INTERVAL_MS-spaced keepalives — OkHttp's normal
            // per-read timeout has no business tearing that down.
            .readTimeout(0, TimeUnit.SECONDS)
            .build()

        /** Opens the WSS connection, sends `hello`, and waits for `hello_ack`. */
        suspend fun connect(
            baseUrl: String,
            accessToken: String,
            deviceId: String,
            capabilities: CapabilityProfile? = null,
            http: OkHttpClient = defaultClient,
        ): Pair<SignalingClient, ReceiveChannel<SignalMessage>> {
            val wsUrl = toWsUrl(baseUrl)
            val inbound = Channel<SignalMessage>(Channel.UNLIMITED)
            val handshake = CompletableDeferred<SignalMessage.HelloAck>()

            val listener = object : WebSocketListener() {
                override fun onOpen(webSocket: WebSocket, response: Response) {
                    val hello = SignalMessage.Hello(
                        protocolVersion = PROTOCOL_VERSION,
                        accessToken = accessToken,
                        deviceId = deviceId,
                        capabilities = capabilities,
                    )
                    webSocket.send(SwarmJson.encodeToString<SignalMessage>(hello))
                }

                override fun onMessage(webSocket: WebSocket, text: String) {
                    val message = runCatching { SwarmJson.decodeFromString<SignalMessage>(text) }.getOrNull() ?: return
                    if (!handshake.isCompleted) {
                        when (message) {
                            is SignalMessage.HelloAck -> handshake.complete(message)
                            is SignalMessage.Error ->
                                handshake.completeExceptionally(SignalingError.Rejected(message.code, message.message))
                            else -> {} // anything else before the handshake completes just isn't useful yet
                        }
                        return
                    }
                    when (message) {
                        is SignalMessage.Pong, is SignalMessage.Hello, is SignalMessage.HelloAck -> {}
                        is SignalMessage.Ping ->
                            webSocket.send(SwarmJson.encodeToString<SignalMessage>(SignalMessage.Pong(message.seq)))
                        else -> inbound.trySend(message)
                    }
                }

                override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                    inbound.close()
                    if (!handshake.isCompleted) handshake.completeExceptionally(SignalingError.ConnectionClosed)
                }

                override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                    inbound.close(t)
                    if (!handshake.isCompleted) handshake.completeExceptionally(SignalingError.Connect(t))
                }
            }

            val webSocket = http.newWebSocket(Request.Builder().url(wsUrl).build(), listener)
            val ack = try {
                withTimeout(HELLO_ACK_TIMEOUT_MS) { handshake.await() }
            } catch (e: TimeoutCancellationException) {
                webSocket.cancel()
                throw SignalingError.HelloTimeout
            } catch (e: SignalingError) {
                webSocket.cancel()
                throw e
            }

            val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())
            scope.launch {
                var seq = 0L
                while (true) {
                    delay(PING_INTERVAL_MS)
                    seq += 1
                    webSocket.send(SwarmJson.encodeToString<SignalMessage>(SignalMessage.Ping(seq)))
                }
            }

            return SignalingClient(ack.sessionId, ack.observedAddr, ack.reflectorPorts, webSocket, scope) to inbound
        }

        private fun toWsUrl(baseUrl: String): String {
            val trimmed = baseUrl.trimEnd('/')
            val wsBase = when {
                trimmed.startsWith("https://") -> "wss://" + trimmed.removePrefix("https://")
                trimmed.startsWith("http://") -> "ws://" + trimmed.removePrefix("http://")
                else -> throw SignalingError.InvalidBaseUrl(baseUrl)
            }
            return "$wsBase/api/v1/ws"
        }
    }

    /** Queues a message for delivery. Fire-and-forget, matching OkHttp `WebSocket.send`'s own contract. */
    fun send(message: SignalMessage) {
        webSocket.send(SwarmJson.encodeToString(message))
    }

    /** Convenience for the common case: relay a hole-punch payload to [to]. */
    fun sendSignal(to: String, payload: SignalPayload) {
        send(SignalMessage.Signal(to = to, payload = payload))
    }

    /** Tells the server this session is ending on purpose — purely a courtesy, same as the Rust client. */
    fun bye() {
        send(SignalMessage.Bye)
    }

    /** Stops the keepalive loop and closes the socket. */
    fun close() {
        scope.cancel()
        webSocket.close(1000, null)
    }
}
