/**
 * Onboarding screen: STUN server URL + device name (free text — the one
 * place this app uses phone-style Material3 fields, since there's no D-pad-
 * friendly way to avoid occasional text entry here) and the 8-digit join
 * code, entered via a D-pad-navigable number grid rather than the system
 * keyboard, per the plan's Fire TV UX requirements.
 */
package app.swarm.tv.app.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.ButtonDefaults
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmAccentHot
import app.swarm.tv.app.ui.theme.SwarmBorder
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmSurface
import app.swarm.tv.app.ui.theme.SwarmSurfaceMuted
import app.swarm.tv.app.ui.theme.SwarmText

@Composable
fun PasscodeEntryScreen(
    isSubmitting: Boolean,
    errorMessage: String?,
    onSubmit: (baseUrl: String, code: String, deviceName: String) -> Unit,
) {
    var baseUrl by remember { mutableStateOf("") }
    var code by remember { mutableStateOf("") }
    var deviceName by remember { mutableStateOf("Fire TV") }

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
        Text("SWARM", color = SwarmAccent, fontSize = 32.sp, fontWeight = FontWeight.Black)
        Spacer(Modifier.height(8.dp))
        Text("Join a swarm to stream from your servers", color = SwarmMuted, fontSize = 16.sp)
        Spacer(Modifier.height(32.dp))

        OutlinedTextField(
            value = baseUrl,
            onValueChange = { baseUrl = it },
            label = { Text("STUN server URL") },
            placeholder = { Text("https://swarm.example.com") },
            singleLine = true,
            colors = fieldColors(),
            modifier = Modifier.width(420.dp),
        )
        Spacer(Modifier.height(12.dp))
        OutlinedTextField(
            value = deviceName,
            onValueChange = { deviceName = it },
            label = { Text("Device name") },
            singleLine = true,
            colors = fieldColors(),
            modifier = Modifier.width(420.dp),
        )
        Spacer(Modifier.height(28.dp))

        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            for (i in 0 until 8) {
                DigitSlot(digit = code.getOrNull(i))
            }
        }
        Spacer(Modifier.height(24.dp))

        NumberPad(
            enabled = !isSubmitting,
            onDigit = { d -> if (code.length < 8) code += d },
            onBackspace = { if (code.isNotEmpty()) code = code.dropLast(1) },
        )
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
