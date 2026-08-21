/**
 * Onboarding screen: automatically discovered LAN media servers first, with
 * direct reconnect for an already-trusted certificate or a six-digit code
 * for first-time pairing. STUN URL + its eight-digit join code remain below
 * as the remote-network path. Free text uses phone-style Material3 fields
 * because TV Material3 does not provide one; both numeric codes use the
 * D-pad-navigable number grid rather than the system keyboard.
 */
package app.swarm.tv.app.ui.screens

import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.ButtonDefaults
import app.swarm.tv.R
import app.swarm.tv.app.data.LanServer
import app.swarm.tv.app.ui.components.NumberPadEntry
import app.swarm.tv.app.ui.components.TvOutlinedTextField
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmAccentHot
import app.swarm.tv.app.ui.theme.SwarmBorder
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmText

@Composable
fun PasscodeEntryScreen(
    isSubmitting: Boolean,
    errorMessage: String?,
    defaultDeviceName: String,
    lanServers: List<LanServer>,
    lanPairingBusy: Boolean,
    onConnectLan: (server: LanServer, deviceName: String) -> Unit,
    onPairLan: (server: LanServer, code: String, deviceName: String) -> Unit,
    onSubmit: (baseUrl: String, code: String, deviceName: String) -> Unit,
) {
    var baseUrl by remember { mutableStateOf("") }
    var code by remember { mutableStateOf("") }
    var lanCode by remember { mutableStateOf("") }
    var selectedLanService by remember { mutableStateOf<String?>(null) }
    // Resolved from the TV itself (see resolveDeviceName) — still a plain
    // editable field, so a real device name that's wrong/undesired can
    // just be typed over before submitting, same as always.
    var deviceName by remember { mutableStateOf(defaultDeviceName) }

    // verticalScroll, not just Arrangement.Center: on a real 1080p Fire TV
    // this screen's full content (title, both text fields, digit slots, and
    // all 4 number-pad rows) is taller than the viewport. Without a scroll
    // container the bottom rows (7/8/9, 0/backspace) render off-screen and
    // are completely unreachable by D-pad — confirmed on real hardware:
    // LEFT/RIGHT navigate fine, but DOWN past row 2 of the pad never moves
    // focus at all, silently blocking entry of any passcode containing
    // 7/8/9/0. Compose brings the focused item into view automatically as
    // focus moves through a scrollable ancestor, so D-pad navigation alone
    // now reaches every row.
    Column(
        modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(48.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Image(
            painter = painterResource(R.drawable.mascot),
            contentDescription = null,
            modifier = Modifier.height(72.dp),
        )
        Spacer(Modifier.height(8.dp))
        Text("SWARM", color = SwarmAccent, fontSize = 32.sp, fontWeight = FontWeight.Black)
        Text("Stream Whatever, Anywhere — Remote Media", color = SwarmMuted, fontSize = 12.sp)
        Spacer(Modifier.height(8.dp))
        Text("Connect to a media server", color = SwarmMuted, fontSize = 16.sp)
        Spacer(Modifier.height(32.dp))

        TvOutlinedTextField(
            value = deviceName,
            onValueChange = { deviceName = it },
            label = { Text("Device name") },
            colors = fieldColors(),
            modifier = Modifier.width(420.dp),
        )
        Spacer(Modifier.height(24.dp))

        Text("Servers on this network", color = SwarmText, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Spacer(Modifier.height(10.dp))
        if (lanServers.isEmpty()) {
            Text("Searching your LAN…", color = SwarmMuted, fontSize = 14.sp)
        } else {
            lanServers.forEach { server ->
                Column(modifier = Modifier.width(520.dp).padding(vertical = 8.dp)) {
                    Text(server.name, color = SwarmText, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                    Text("${server.host}:${server.peerPort}", color = SwarmMuted, fontSize = 12.sp)
                    Spacer(Modifier.height(7.dp))
                    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        Button(
                            onClick = { onConnectLan(server, deviceName) },
                            enabled = !lanPairingBusy && !isSubmitting,
                            colors = ButtonDefaults.colors(containerColor = SwarmAccent, contentColor = Color(0xFF04263A)),
                        ) { Text(if (lanPairingBusy) "Connecting…" else "Connect", fontWeight = FontWeight.Bold) }
                        Button(
                            onClick = {
                                selectedLanService = server.serviceName
                                lanCode = ""
                            },
                            enabled = !lanPairingBusy && !isSubmitting,
                        ) { Text("Pair first time") }
                    }
                }
            }
        }

        val selectedLanServer = lanServers.firstOrNull { it.serviceName == selectedLanService }
        if (selectedLanServer != null) {
            Spacer(Modifier.height(14.dp))
            Text("LAN pairing code for ${selectedLanServer.name}", color = SwarmMuted, fontSize = 14.sp)
            Spacer(Modifier.height(10.dp))
            NumberPadEntry(value = lanCode, maxLength = 6, onValueChange = { lanCode = it }, enabled = !lanPairingBusy)
            Spacer(Modifier.height(14.dp))
            Button(
                onClick = { onPairLan(selectedLanServer, lanCode, deviceName) },
                enabled = !lanPairingBusy && lanCode.length == 6,
                colors = ButtonDefaults.colors(containerColor = SwarmAccent, contentColor = Color(0xFF04263A)),
            ) { Text(if (lanPairingBusy) "Pairing…" else "Pair and connect", fontWeight = FontWeight.Bold) }
        }

        Spacer(Modifier.height(36.dp))
        Text("Or connect through a STUN server", color = SwarmText, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Spacer(Modifier.height(12.dp))

        TvOutlinedTextField(
            value = baseUrl,
            onValueChange = { baseUrl = it },
            label = { Text("STUN server URL") },
            placeholder = { Text("https://swarm.example.com") },
            colors = fieldColors(),
            modifier = Modifier.width(420.dp),
        )
        Spacer(Modifier.height(28.dp))

        NumberPadEntry(value = code, maxLength = 8, onValueChange = { code = it }, enabled = !isSubmitting)
        Spacer(Modifier.height(28.dp))

        Button(
            onClick = { onSubmit(baseUrl, code, deviceName) },
            enabled = !isSubmitting && code.length == 8 && baseUrl.isNotBlank(),
            colors = ButtonDefaults.colors(containerColor = SwarmAccent, contentColor = Color(0xFF04263A)),
        ) {
            Text(if (isSubmitting) "Joining…" else "Join swarm", fontWeight = FontWeight.Bold)
        }

        errorMessage?.let {
            Spacer(Modifier.height(16.dp))
            Text(it, color = SwarmAccentHot, fontSize = 14.sp)
        }
    }
}

@Composable
private fun fieldColors() = OutlinedTextFieldDefaults.colors(
    focusedTextColor = SwarmText,
    unfocusedTextColor = SwarmText,
    focusedBorderColor = SwarmAccent,
    unfocusedBorderColor = SwarmBorder,
    focusedLabelColor = SwarmAccent,
    unfocusedLabelColor = SwarmMuted,
    cursorColor = SwarmAccent,
)
