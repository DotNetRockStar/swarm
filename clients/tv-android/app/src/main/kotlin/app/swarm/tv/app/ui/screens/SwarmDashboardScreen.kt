/**
 * Post-onboarding screen: the joined swarm and its device roster. This is
 * as far as the client goes until the peer QUIC transport lands — the
 * merged multi-server catalog and player screens (per the plan's Phase 3
 * scope) build directly on the [app.swarm.tv.core.rest.SwarmDevice] list
 * shown here, once each server can be dialed.
 */
package app.swarm.tv.app.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.ButtonDefaults
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import app.swarm.tv.core.rest.DeviceType
import app.swarm.tv.core.rest.SwarmDevice
import app.swarm.tv.core.rest.SwarmSummary
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmGreen
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmSurface
import app.swarm.tv.app.ui.theme.SwarmSurfaceMuted
import app.swarm.tv.app.ui.theme.SwarmText

@Composable
fun SwarmDashboardScreen(
    swarm: SwarmSummary,
    devices: List<SwarmDevice>,
    resyncing: Boolean,
    onResync: () -> Unit,
) {
    Column(modifier = Modifier.fillMaxSize().padding(40.dp)) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column {
                Text("SWARM", color = SwarmAccent, fontSize = 22.sp, fontWeight = FontWeight.Black)
                Text(swarm.name, color = SwarmText, fontSize = 16.sp)
            }
            Button(
                onClick = onResync,
                enabled = !resyncing,
                colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText),
            ) {
                Text(if (resyncing) "Resyncing…" else "Resync")
            }
        }
        Spacer(Modifier.height(24.dp))

        val servers = devices.filter { it.deviceType == DeviceType.SERVER || it.deviceType == DeviceType.BOTH }
        Text("Servers in this swarm (${servers.size})", color = SwarmMuted, fontSize = 14.sp)
        Spacer(Modifier.height(12.dp))

        if (servers.isEmpty()) {
            Text(
                "No servers here yet — join code a SWARM server app onto this swarm to start streaming.",
                color = SwarmMuted,
                fontSize = 14.sp,
            )
        } else {
            LazyColumn(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                items(servers) { device -> ServerRow(device) }
            }
        }
    }
}

@Composable
private fun ServerRow(device: SwarmDevice) {
    Card(onClick = {}, colors = CardDefaults.colors(containerColor = SwarmSurface)) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Column {
                Text(device.name, color = SwarmText, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                Text(device.metadata["hostname"] ?: device.deviceId, color = SwarmMuted, fontSize = 12.sp)
            }
            StatusPill(online = device.online)
        }
    }
}

@Composable
private fun StatusPill(online: Boolean) {
    Row(
        modifier = Modifier
            .clip(RoundedCornerShape(999.dp))
            .background(if (online) SwarmGreen.copy(alpha = 0.18f) else SwarmSurfaceMuted)
            .padding(horizontal = 10.dp, vertical = 4.dp),
    ) {
        Text(if (online) "online" else "offline", color = if (online) SwarmGreen else SwarmMuted, fontSize = 12.sp)
    }
}
