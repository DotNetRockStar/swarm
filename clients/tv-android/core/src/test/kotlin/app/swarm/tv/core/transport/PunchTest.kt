package app.swarm.tv.core.transport

import app.swarm.tv.core.signal.PUNCH_MAGIC
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetSocketAddress
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Test

class PunchTest {
    @Test
    fun `simultaneous punch succeeds both directions`() = runBlocking {
        DatagramSocket(0).use { a ->
            DatagramSocket(0).use { b ->
                val aAddr = InetSocketAddress("127.0.0.1", a.localPort)
                val bAddr = InetSocketAddress("127.0.0.1", b.localPort)
                coroutineScope {
                    val aResult = async { punch(a, listOf(bAddr)) }
                    val bResult = async { punch(b, listOf(aAddr)) }
                    assertEquals(bAddr, aResult.await())
                    assertEquals(aAddr, bResult.await())
                }
            }
        }
    }

    @Test
    fun `tries every candidate each round not just the first`() = runBlocking {
        DatagramSocket(0).use { listener ->
            val listenerAddr = InetSocketAddress("127.0.0.1", listener.localPort)
            val deadCandidate = InetSocketAddress("127.0.0.1", 1) // nothing responds here

            DatagramSocket(0).use { puncher ->
                coroutineScope {
                    val listenTask = async(Dispatchers.IO) {
                        val buffer = ByteArray(64)
                        val packet = DatagramPacket(buffer, buffer.size)
                        listener.receive(packet)
                        assertEquals(PUNCH_MAGIC.toList(), buffer.copyOf(packet.length).toList())
                        val from = packet.socketAddress as InetSocketAddress
                        listener.send(DatagramPacket(PUNCH_MAGIC, PUNCH_MAGIC.size, from))
                    }
                    val result = punch(puncher, listOf(deadCandidate, listenerAddr))
                    assertEquals(listenerAddr, result)
                    listenTask.await()
                }
            }
        }
    }

    @Test
    fun `no response is a no response error`() {
        DatagramSocket(0).use { socket ->
            val nobody = InetSocketAddress("127.0.0.1", 1)
            assertThrows(PunchError.NoResponse::class.java) {
                runBlocking { punch(socket, listOf(nobody)) }
            }
        }
    }

    @Test
    fun `magic from an unlisted address is ignored`() {
        DatagramSocket(0).use { puncher ->
            val intendedCandidate = InetSocketAddress("127.0.0.1", 1) // never actually responds
            DatagramSocket(0).use { stranger ->
                val puncherAddr = InetSocketAddress("127.0.0.1", puncher.localPort)
                stranger.send(DatagramPacket(PUNCH_MAGIC, PUNCH_MAGIC.size, puncherAddr))
                assertThrows(PunchError.NoResponse::class.java) {
                    runBlocking { punch(puncher, listOf(intendedCandidate)) }
                }
            }
        }
    }
}
