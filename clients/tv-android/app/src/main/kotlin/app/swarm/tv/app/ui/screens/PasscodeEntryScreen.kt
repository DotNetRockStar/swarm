/**
 * Onboarding screen: STUN server URL + device name (free text — the one
 * place this app uses phone-style Material3 fields, since there's no D-pad-
 * friendly way to avoid occasional text entry here) and the 8-digit join
 * code, entered via a D-pad-navigable number grid rather than the system
 * keyboard, per the plan's Fire TV UX requirements.
 */
package app.swarm.tv.app.ui.screens

import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
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
    onSubmit: (baseUrl: String, code: String, deviceName: String) -> Unit,
) {
    var baseUrl by remember { mutableStateOf("") }
    var code by remember { mutableStateOf("") }
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
        Text("Join a swarm to stream from your servers", color = SwarmMuted, fontSize = 16.sp)
        Spacer(Modifier.height(32.dp))

        TvOutlinedTextField(
            value = baseUrl,
            onValueChange = { baseUrl = it },
            label = { Text("STUN server URL") },
            placeholder = { Text("https://swarm.example.com") },
            colors = fieldColors(),
            modifier = Modifier.width(420.dp),
        )
        Spacer(Modifier.height(12.dp))
        TvOutlinedTextField(
            value = deviceName,
            onValueChange = { deviceName = it },
            label = { Text("Device name") },
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

