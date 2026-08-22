package app.swarm.tv.core.transport

import java.io.ByteArrayInputStream
import java.io.IOException
import java.io.InputStream
import org.junit.jupiter.api.Assertions.assertArrayEquals
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class BoundedInputStreamTest {
    @Test
    fun `unexpected eof reports received and expected byte counts`() {
        val stream = BoundedInputStream(ByteArrayInputStream(byteArrayOf(1, 2, 3)), 10)

        val error = assertThrows(PeerQuicError.TruncatedBody::class.java) { stream.readBytes() }

        assertEquals(3, error.gotBytes)
        assertEquals(10, error.expectedBytes)
    }

    @Test
    fun `transport error reports where the body was interrupted`() {
        val delegate = object : InputStream() {
            private val bytes = byteArrayOf(1, 2, 3)
            private var index = 0

            override fun read(): Int {
                if (index == bytes.size) throw IOException("connection closed")
                return bytes[index++].toInt()
            }
        }
        val stream = BoundedInputStream(delegate, 10)

        val error = assertThrows(IOException::class.java) { stream.readBytes() }

        assertTrue(error.message.orEmpty().contains("3/10 bytes"))
        assertTrue(error.message.orEmpty().contains("connection closed"))
    }

    @Test
    fun `reads exactly the advertised body length`() {
        val stream = BoundedInputStream(ByteArrayInputStream(byteArrayOf(1, 2, 3, 4, 5)), 3)

        assertArrayEquals(byteArrayOf(1, 2, 3), stream.readBytes())
        assertEquals(-1, stream.read())
    }
}
