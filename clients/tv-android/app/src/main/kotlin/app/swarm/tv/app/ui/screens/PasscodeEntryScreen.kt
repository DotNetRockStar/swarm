/** TV activation screens used from the main SWARM dashboard. */
package app.swarm.tv.app.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import app.swarm.tv.R
import app.swarm.tv.app.ui.components.SwarmLoadingIndicator
import app.swarm.tv.app.ui.components.swarmActionButtonColors
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmError
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmText

@Composable
fun ActivationRequestScreen(onCancel: () -> Unit) {
    BackHandler(onBack = onCancel)
    Column(
        modifier = Modifier.fillMaxSize().padding(48.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Image(painter = painterResource(R.drawable.mascot), contentDescription = null, modifier = Modifier.height(72.dp))
        Spacer(Modifier.height(18.dp))
        Text("Creating a secure code", color = SwarmText, fontSize = 26.sp, fontWeight = FontWeight.Black)
        Spacer(Modifier.height(18.dp))
        SwarmLoadingIndicator()
        Spacer(Modifier.height(18.dp))
        Text("This should only take a moment.", color = SwarmMuted, fontSize = 15.sp)
        Spacer(Modifier.height(24.dp))
        Button(onClick = onCancel, colors = swarmActionButtonColors()) { Text("Cancel") }
    }
}

@Composable
fun ActivationCodeScreen(
    code: String,
    expiresAt: String,
    errorMessage: String?,
    onCancel: () -> Unit,
) {
    BackHandler(onBack = onCancel)
    Column(
        modifier = Modifier.fillMaxSize().padding(48.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Image(painter = painterResource(R.drawable.mascot), contentDescription = null, modifier = Modifier.height(72.dp))
        Spacer(Modifier.height(18.dp))
        Text("Approve this TV", color = SwarmText, fontSize = 26.sp, fontWeight = FontWeight.Black)
        Spacer(Modifier.height(10.dp))
        Text("Enter this temporary code on the media server's Swarm page", color = SwarmMuted, fontSize = 15.sp)
        Spacer(Modifier.height(26.dp))
        Text(code.chunked(4).joinToString("  "), color = SwarmAccent, fontSize = 46.sp, fontWeight = FontWeight.Black)
        Spacer(Modifier.height(14.dp))
        Text("Waiting for approval · expires $expiresAt", color = SwarmMuted, fontSize = 13.sp)
        errorMessage?.let {
            Spacer(Modifier.height(12.dp))
            Text(it, color = SwarmError, fontSize = 14.sp)
        }
        Spacer(Modifier.height(24.dp))
        Button(onClick = onCancel, colors = swarmActionButtonColors()) { Text("Cancel") }
    }
}
