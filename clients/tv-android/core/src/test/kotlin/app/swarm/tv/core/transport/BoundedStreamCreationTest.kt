package app.swarm.tv.core.transport

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertSame
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import tech.kwik.core.QuicStream

/**
 * Regression coverage for #140: at the very end of a long movie the media
 * server stopped advancing MAX_STREAMS, so kwik's `createStream(true)` — which
 * blocks for up to 10,000 days when the peer has issued no stream credit —
 * parked every following request (catalog, playback, artwork) forever, and
 * closing the connection could not release it. [openBoundedStream] converts
 * that into a prompt [IOException] the reconnect path can recover from.
 */
class BoundedStreamCreationTest {
    @Test
    fun `a credit-starved createStream fails fast instead of hanging and the blocked call is interrupted`() {
        val started = CountDownLatch(1)
        val interrupted = CountDownLatch(1)

        val startedAt = System.nanoTime()
        val error = assertThrows(IOException::class.java) {
            openBoundedStream(
                create = {
                    started.countDown()
                    try {
                        // kwik's real hard-coded default when no stream credit is available.
                        Thread.sleep(TimeUnit.DAYS.toMillis(10_000))
                    } catch (e: InterruptedException) {
                        interrupted.countDown()
                        throw e
                    }
                    error("unreachable: the blocking wait must be abandoned")
                },
                timeoutMs = 200L,
            )
        }
        val elapsedMs = (System.nanoTime() - startedAt) / 1_000_000

        assertTrue(started.await(2, TimeUnit.SECONDS), "the create call must have run")
        assertTrue(error.message!!.contains("stalled"), "unexpected message: ${error.message}")
        assertTrue(elapsedMs < 5_000, "must not wait on the underlying blocking call (waited ${elapsedMs}ms)")
        assertTrue(
            interrupted.await(2, TimeUnit.SECONDS),
            "the abandoned create call must be interrupted so its worker thread is not leaked",
        )
    }

    @Test
    fun `a real transport failure during stream creation propagates unchanged`() {
        val error = assertThrows(IOException::class.java) {
            openBoundedStream(create = { throw IOException("not connected") })
        }
        assertEquals("not connected", error.message)
    }

    @Test
    fun `an unchecked kwik failure during stream creation is normalized to IOException`() {
        val error = assertThrows(IOException::class.java) {
            openBoundedStream(create = { throw IllegalStateException("connection closed") })
        }
        assertTrue(error.message!!.contains("connection closed"), "unexpected message: ${error.message}")
    }

    @Test
    fun `a healthy stream creation passes straight through`() {
        val stream = FakeQuicStream()
        assertSame(stream, openBoundedStream(create = { stream }))
        assertFalse(stream.aborted)
    }
}

private class FakeQuicStream : QuicStream {
    var aborted = false

    override fun getInputStream(): InputStream = ByteArrayInputStream(ByteArray(0))
    override fun getOutputStream(): OutputStream = ByteArrayOutputStream()
    override fun getStreamId(): Int = 0
    override fun isUnidirectional(): Boolean = false
    override fun isClientInitiatedBidirectional(): Boolean = true
    override fun isServerInitiatedBidirectional(): Boolean = false
    override fun abortReading(applicationProtocolErrorCode: Long) { aborted = true }
    override fun resetStream(applicationProtocolErrorCode: Long) { aborted = true }
}
