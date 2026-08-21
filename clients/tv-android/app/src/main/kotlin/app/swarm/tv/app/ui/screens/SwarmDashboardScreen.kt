/**
 * Post-onboarding screen: the joined swarm and its device roster, plus the
 * entry point into [app.swarm.tv.app.ui.screens.CatalogScreen] — which
 * connects to these same servers via their self-reported `peer_addr` and
 * merges their catalogs (see [app.swarm.tv.core.catalog.CatalogSession]).
 */
package app.swarm.tv.app.ui.screens

import android.app.Activity
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
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
import app.swarm.tv.app.ui.theme.SwarmAccentHot
import app.swarm.tv.app.ui.theme.SwarmBackground
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
    onBrowseCatalog: () -> Unit,
    onOpenSettings: () -> Unit,
) {
    // This screen is reached by swapping UiState branches (see SwarmApp's
    // `when`), not a real navigation component — nothing else ever moves
    // D-pad focus onto its content. Confirmed on real hardware: without an
    // explicit request here, no element on this screen ever receives
    // focus at all, so D-pad input is silently a no-op forever (the
    // buttons still render as if focused thanks to their static accent
    // color, making this invisible from a screenshot — only proven by
    // instrumenting the click handlers and seeing them never fire).
    val browseLibraryFocusRequester = remember { FocusRequester() }
    LaunchedEffect(Unit) { browseLibraryFocusRequester.requestFocus() }

    // This is the app's true home screen — every other screen's Back chain
    // (Settings, Catalog, detail screens, ...) ultimately lands here, so
    // this is the one place a physical Back press would otherwise fall
    // through to Android's default behavior and silently kill the Activity.
    // That's the accidental-exit bug real use surfaced: Back one time too
    // many while browsing quits the whole app with no warning. Trapping it
    // here with a confirm step (rather than on every individual screen)
    // keeps the fix in one place matching where the real risk actually is.
    var showExitConfirm by remember { mutableStateOf(false) }
    val activity = LocalContext.current as? Activity

    BackHandler(enabled = !showExitConfirm) { showExitConfirm = true }

    Box(modifier = Modifier.fillMaxSize()) {
        Column(
            modifier = Modifier.fillMaxSize().padding(40.dp)
                // Same reasoning as CatalogScreen's FilterOverlay: without this,
                // D-pad focus can still travel into this content from behind the
                // (visually on-top) exit-confirm overlay.
                .focusProperties { canFocus = !showExitConfirm },
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column {
                    Text("SWARM", color = SwarmAccent, fontSize = 22.sp, fontWeight = FontWeight.Black)
                    Text(swarm.name, color = SwarmText, fontSize = 16.sp)
                }
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Button(
                        onClick = onBrowseCatalog,
                        modifier = Modifier.focusRequester(browseLibraryFocusRequester),
                        colors = ButtonDefaults.colors(containerColor = SwarmAccent, contentColor = SwarmBackground),
                    ) {
                        Text("Browse library")
                    }
                    Button(
                        onClick = onResync,
                        enabled = !resyncing,
                        colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText),
                    ) {
                        Text(if (resyncing) "Resyncing…" else "Resync")
                    }
                    Button(
                        onClick = onOpenSettings,
                        colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText),
                    ) {
                        Text("Settings")
                    }
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

        if (showExitConfirm) {
            ExitConfirmOverlay(
                onConfirmExit = { activity?.finish() },
                onDismiss = { showExitConfirm = false },
            )
        }
    }
}

/**
 * A single confirm action, no on-screen Cancel — same reasoning every other
 * overlay in this app drops one: the physical Back button already dismisses
 * this (wired below via [BackHandler]), and it's the more natural gesture
 * anyway since Back is exactly what the user just pressed to get here.
 */
@Composable
private fun ExitConfirmOverlay(onConfirmExit: () -> Unit, onDismiss: () -> Unit) {
    BackHandler(onBack = onDismiss)
    val exitFocusRequester = remember { FocusRequester() }
    LaunchedEffect(Unit) { exitFocusRequester.requestFocus() }

    Box(
        modifier = Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.85f)),
        contentAlignment = Alignment.Center,
    ) {
        Column(
            modifier = Modifier.clip(RoundedCornerShape(16.dp)).background(SwarmSurface).padding(28.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text("Exit SWARM?", color = SwarmText, fontSize = 20.sp, fontWeight = FontWeight.Black)
            Spacer(Modifier.height(8.dp))
            Text("Press Back again to stay.", color = SwarmMuted, fontSize = 13.sp)
            Spacer(Modifier.height(20.dp))
            Button(
                onClick = onConfirmExit,
                modifier = Modifier.focusRequester(exitFocusRequester),
                colors = ButtonDefaults.colors(
                    containerColor = SwarmAccentHot,
                    contentColor = SwarmText,
                    focusedContainerColor = SwarmAccentHot,
                    focusedContentColor = SwarmText,
                    pressedContainerColor = SwarmAccentHot,
                    pressedContentColor = SwarmText,
                ),
            ) {
                Text("Exit app", fontWeight = FontWeight.Bold)
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
