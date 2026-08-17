/**
 * Local network address detection, mirroring
 * `swarm_p2p::local_addr::detect_local_ipv4` (Rust): UDP `connect()` to a
 * well-known address and read back the socket's local address. UDP
 * `connect()` only consults the OS routing table to pick a source address
 * — it never actually sends a packet — so this works even fully offline
 * and carries no privacy/network cost.
 */
package app.swarm.tv.core.transport

import java.net.DatagramSocket
import java.net.InetAddress

private const val PROBE_HOST = "8.8.8.8"
private const val PROBE_PORT = 80

/** Best-guess LAN-facing IPv4 address. Falls back to loopback if no route exists (offline, sandboxed). */
fun detectLocalIpv4(): InetAddress =
    runCatching {
        DatagramSocket().use { socket ->
            socket.connect(InetAddress.getByName(PROBE_HOST), PROBE_PORT)
            socket.localAddress
        }
    }.getOrDefault(InetAddress.getLoopbackAddress())
