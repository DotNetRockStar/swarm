package app.swarm.tv.app.data

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.Looper
import app.swarm.tv.core.rest.DeviceType
import app.swarm.tv.core.rest.SwarmDevice
import app.swarm.tv.core.rest.SwarmJson
import java.io.Closeable
import java.net.InetSocketAddress
import java.net.Socket
import java.util.ArrayDeque
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.decodeFromString
import kotlinx.serialization.encodeToString

/** A media server resolved from SWARM's DNS-SD advertisement. */
data class LanServer(
    val serviceName: String,
    val name: String,
    val host: String,
    val peerPort: Int,
    val pairingPort: Int,
    val certFingerprint: String,
) {
    fun asSwarmDevice(): SwarmDevice {
        val address = if (host.contains(':')) "[$host]:$peerPort" else "$host:$peerPort"
        return SwarmDevice(
            deviceId = "lan-${certFingerprint.take(16)}",
            name = name,
            deviceType = DeviceType.SERVER,
            certFingerprint = certFingerprint,
            online = true,
            metadata = mapOf("peer_addr" to address, "hostname" to host),
        )
    }
}

/**
 * Discovers `_swarm-peer._udp` services using Android's native DNS-SD API.
 * A multicast lock is held only while discovery is active; some Fire TV and
 * Android TV Wi-Fi drivers otherwise filter mDNS packets before NSD sees them.
 */
class LanDiscoveryManager(context: Context) : Closeable {
    companion object {
        private const val SERVICE_TYPE = "_swarm-peer._udp."
    }

    private val nsd = context.getSystemService(Context.NSD_SERVICE) as NsdManager
    private val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
    private val mainHandler = Handler(Looper.getMainLooper())
    private val resolved = linkedMapOf<String, LanServer>()
    private val resolveQueue = ArrayDeque<NsdServiceInfo>()
    private var resolving = false
    private var started = false
    private var multicastLock: WifiManager.MulticastLock? = null

    private val _servers = MutableStateFlow<List<LanServer>>(emptyList())
    val servers: StateFlow<List<LanServer>> = _servers.asStateFlow()

    private val discoveryListener = object : NsdManager.DiscoveryListener {
        override fun onDiscoveryStarted(serviceType: String) = Unit

        override fun onServiceFound(serviceInfo: NsdServiceInfo) {
            if (serviceInfo.serviceType.startsWith(SERVICE_TYPE)) {
                resolveQueue.addLast(serviceInfo)
                resolveNext()
            }
        }

        override fun onServiceLost(serviceInfo: NsdServiceInfo) {
            resolved.remove(serviceInfo.serviceName)
            publish()
        }

        override fun onDiscoveryStopped(serviceType: String) = Unit
        override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) = stopAfterFailure()
        override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) = stopAfterFailure()
    }

    fun start() {
        mainHandler.post {
            if (started) return@post
            started = true
            multicastLock = wifi?.createMulticastLock("swarm-lan-discovery")?.apply {
                setReferenceCounted(false)
                acquire()
            }
            runCatching { nsd.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, discoveryListener) }
                .onFailure { stopAfterFailure() }
        }
    }

    @Suppress("DEPRECATION")
    private fun resolveNext() {
        if (resolving || resolveQueue.isEmpty()) return
        resolving = true
        val service = resolveQueue.removeFirst()
        nsd.resolveService(service, object : NsdManager.ResolveListener {
            override fun onResolveFailed(serviceInfo: NsdServiceInfo, errorCode: Int) = finishResolve()

            override fun onServiceResolved(serviceInfo: NsdServiceInfo) {
                parse(serviceInfo)?.let { resolved[serviceInfo.serviceName] = it }
                publish()
                finishResolve()
            }
        })
    }

    private fun finishResolve() {
        resolving = false
        resolveNext()
    }

    private fun parse(info: NsdServiceInfo): LanServer? {
        fun attribute(name: String): String? = info.attributes[name]?.toString(Charsets.UTF_8)
        val fingerprint = attribute("fingerprint")?.lowercase() ?: return null
        if (fingerprint.length != 64 || fingerprint.any { !it.isDigit() && it !in 'a'..'f' }) return null
        val host = info.host?.hostAddress ?: return null
        val peerPort = attribute("peer_port")?.toIntOrNull() ?: info.port
        val pairingPort = attribute("pair_port")?.toIntOrNull() ?: return null
        if (peerPort !in 1..65535 || pairingPort !in 1..65535) return null
        return LanServer(
            serviceName = info.serviceName,
            name = attribute("name")?.takeIf(String::isNotBlank) ?: info.serviceName,
            host = host,
            peerPort = peerPort,
            pairingPort = pairingPort,
            certFingerprint = fingerprint,
        )
    }

    private fun publish() {
        _servers.value = resolved.values.sortedBy { it.name.lowercase() }
    }

    private fun stopAfterFailure() {
        started = false
        resolving = false
        resolveQueue.clear()
        multicastLock?.takeIf { it.isHeld }?.release()
        multicastLock = null
    }

    override fun close() {
        mainHandler.post {
            if (started) runCatching { nsd.stopServiceDiscovery(discoveryListener) }
            stopAfterFailure()
            resolved.clear()
            publish()
        }
    }

    suspend fun pair(server: LanServer, code: String, deviceName: String, fingerprint: String): Result<Unit> =
        withContext(Dispatchers.IO) {
            runCatching {
                Socket().use { socket ->
                    socket.connect(InetSocketAddress(server.host, server.pairingPort), 5_000)
                    socket.soTimeout = 5_000
                    val request = LanPairRequest(code, deviceName, fingerprint)
                    socket.getOutputStream().bufferedWriter().use { writer ->
                        writer.write(SwarmJson.encodeToString(request))
                        writer.newLine()
                        writer.flush()
                        val line = socket.getInputStream().bufferedReader().readLine()
                            ?: error("The media server closed the pairing connection.")
                        val response = SwarmJson.decodeFromString<LanPairResponse>(line)
                        if (!response.ok) {
                            val message = when (response.error) {
                                "pairing_closed_or_bad_code" -> "The LAN pairing code is incorrect or expired."
                                "not_lan" -> "The media server did not recognize this as a LAN connection."
                                else -> "The media server rejected the pairing request."
                            }
                            error(message)
                        }
                    }
                }
            }
        }
}

@Serializable
private data class LanPairRequest(val code: String, val name: String, val fingerprint: String)

@Serializable
private data class LanPairResponse(val ok: Boolean, val error: String? = null)
