/**
 * Multi-swarm membership management: reached from [SwarmDashboardScreen],
 * lists every swarm this device belongs to, lets the user switch which one
 * is active, leave one, or join an additional one with a fresh 8-digit code
 * (same D-pad number-grid pattern as [PasscodeEntryScreen], reimplemented
 * locally rather than shared since that screen's fields — STUN URL, device
 * name — don't apply once a device is already registered).
 */
package app.swarm.tv.app.ui.screens

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
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
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
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.ButtonDefaults
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmAccentHot
import app.swarm.tv.app.ui.theme.SwarmGreen
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmSurface
import app.swarm.tv.app.ui.theme.SwarmSurfaceMuted
import app.swarm.tv.app.ui.theme.SwarmText
import app.swarm.tv.core.rest.SwarmSummary

@Composable
fun SwarmSettingsScreen(
    allSwarms: List<SwarmSummary>,
    activeSwarmId: String?,
    busy: Boolean,
    errorMessage: String?,
    onJoin: (code: String) -> Unit,
    onLeave: (swarmId: String) -> Unit,
    onSwitchActive: (swarmId: String) -> Unit,
    onBack: () -> Unit,
) {
    var code by remember { mutableStateOf("") }

    // Same reasoning as SwarmDashboardScreen: reached by a UiState swap, not
    // real navigation, so nothing else ever moves D-pad focus here.
    val backFocusRequester = remember { FocusRequester() }
    LaunchedEffect(Unit) { backFocusRequester.requestFocus() }

    Column(modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(40.dp)) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("Swarm settings", color = SwarmAccent, fontSize = 22.sp, fontWeight = FontWeight.Black)
            Button(
                onClick = onBack,
                modifier = Modifier.focusRequester(backFocusRequester),
                colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText),
            ) {
                Text("Back")
            }
        }
        Spacer(Modifier.height(24.dp))

        Text("Joined swarms (${allSwarms.size})", color = SwarmMuted, fontSize = 14.sp)
        Spacer(Modifier.height(12.dp))
        if (allSwarms.isEmpty()) {
            Text("Not a member of any swarm yet — join one below.", color = SwarmMuted, fontSize = 14.sp)
        } else {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                for (swarm in allSwarms) {
                    SwarmRow(
                        swarm = swarm,
                        isActive = swarm.id == activeSwarmId,
                        busy = busy,
                        onSelect = { onSwitchActive(swarm.id) },
                        onLeave = { onLeave(swarm.id) },
                    )
                }
            }
        }

        Spacer(Modifier.height(32.dp))
        Text("Join another swarm", color = SwarmMuted, fontSize = 14.sp)
        Spacer(Modifier.height(12.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            for (i in 0 until 8) DigitSlot(code.getOrNull(i))
        }
        Spacer(Modifier.height(16.dp))
        NumberPad(
            enabled = !busy,
            onDigit = { d -> if (code.length < 8) code += d },
            onBackspace = { if (code.isNotEmpty()) code = code.dropLast(1) },
        )
        Spacer(Modifier.height(16.dp))
        Button(
            onClick = { onJoin(code); code = "" },
            enabled = !busy && code.length == 8,
            colors = ButtonDefaults.colors(containerColor = SwarmAccent, contentColor = Color(0xFF04263A)),
        ) {
            Text(if (busy) "Working…" else "Join swarm", fontWeight = FontWeight.Bold)
        }

        errorMessage?.let {
            Spacer(Modifier.height(16.dp))
            Text(it, color = SwarmAccentHot, fontSize = 14.sp)
        }
    }
}

@Composable
private fun SwarmRow(swarm: SwarmSummary, isActive: Boolean, busy: Boolean, onSelect: () -> Unit, onLeave: () -> Unit) {
    // Two independently-reachable D-pad targets, not one Card wrapping a
    // nested Button — a clickable Card's own focus/click semantics swallow
    // D-pad traversal to children nested inside it, making the inner
    // control unreachable (confirmed on real hardware: RIGHT/UP/DOWN off
    // the Card never focused the nested "Leave" button). Both the select
    // surface and the Leave button are direct children of this Row instead,
    // so LEFT/RIGHT between them works via normal 2D focus search.
    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Card(
            onClick = onSelect,
            modifier = Modifier.weight(1f),
            colors = CardDefaults.colors(containerColor = if (isActive) SwarmSurfaceMuted else SwarmSurface),
        ) {
            Column(modifier = Modifier.fillMaxWidth().padding(16.dp)) {
                Text(swarm.name, color = SwarmText, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                if (isActive) Text("active", color = SwarmGreen, fontSize = 12.sp)
            }
        }
        Button(
            onClick = onLeave,
            enabled = !busy,
            colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmAccentHot),
        ) {
            Text("Leave")
        }
    }
}

@Composable
private fun DigitSlot(digit: Char?) {
    Box(
        modifier = Modifier
            .size(40.dp, 52.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(if (digit != null) SwarmSurfaceMuted else SwarmSurface),
        contentAlignment = Alignment.Center,
    ) {
        Text(digit?.toString() ?: "", color = SwarmText, fontSize = 24.sp, fontWeight = FontWeight.Bold)
    }
}

private val padRows = listOf(
    listOf('1', '2', '3'),
    listOf('4', '5', '6'),
    listOf('7', '8', '9'),
    listOf(null, '0', '<'),
)

@Composable
private fun NumberPad(enabled: Boolean, onDigit: (Char) -> Unit, onBackspace: () -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        for (row in padRows) {
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                for (key in row) {
                    Box(modifier = Modifier.size(64.dp)) {
                        if (key != null) {
                            Button(
                                onClick = { if (key == '<') onBackspace() else onDigit(key) },
                                enabled = enabled,
                                colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText),
                                modifier = Modifier.size(64.dp),
                            ) {
                                Text(if (key == '<') "⌫" else key.toString(), fontSize = 22.sp)
                            }
                        }
                    }
                }
            }
        }
    }
}
