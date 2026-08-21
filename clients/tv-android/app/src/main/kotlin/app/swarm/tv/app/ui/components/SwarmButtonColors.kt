package app.swarm.tv.app.ui.components

import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.tv.material3.ButtonDefaults
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmAccentHot
import app.swarm.tv.app.ui.theme.SwarmMuted

/** Shared D-pad interaction colors for every conventional action button. */
@Composable
fun swarmActionButtonColors() = ButtonDefaults.colors(
    containerColor = Color.White,
    contentColor = Color(0xFF04263A),
    focusedContainerColor = SwarmAccent,
    focusedContentColor = Color(0xFF04263A),
    pressedContainerColor = SwarmAccentHot,
    pressedContentColor = Color(0xFF3A0420),
    disabledContainerColor = SwarmMuted.copy(alpha = 0.45f),
    disabledContentColor = Color(0xFF101828).copy(alpha = 0.65f),
)
