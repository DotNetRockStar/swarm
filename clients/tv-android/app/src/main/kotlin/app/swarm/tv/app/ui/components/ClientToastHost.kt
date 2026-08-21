package app.swarm.tv.app.ui.components

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.swarm.tv.app.data.ClientNotification
import app.swarm.tv.app.data.ClientNotificationKind
import app.swarm.tv.app.ui.theme.SwarmBorder
import app.swarm.tv.app.ui.theme.SwarmGreen
import app.swarm.tv.app.ui.theme.SwarmSurface
import app.swarm.tv.app.ui.theme.SwarmText
import kotlinx.coroutines.delay

internal data class VisibleToast(val id: Long, val notification: ClientNotification)

@Stable
class ClientToastHostState internal constructor() {
    internal val toasts = mutableStateListOf<VisibleToast>()
    private var nextId by mutableLongStateOf(0L)

    fun show(notification: ClientNotification) {
        // Collapse repeated callbacks for the same failure into one refreshed toast.
        toasts.removeAll { it.notification == notification }
        toasts += VisibleToast(++nextId, notification)
        while (toasts.size > 4) toasts.removeAt(0)
    }

    internal fun dismiss(id: Long) {
        toasts.removeAll { it.id == id }
    }
}

@Composable
fun rememberClientToastHostState(): ClientToastHostState = remember { ClientToastHostState() }

/** Non-focusable, queued TV notifications modeled after the media server's bottom-right toast stack. */
@Composable
fun ClientToastHost(state: ClientToastHostState, modifier: Modifier = Modifier) {
    Box(modifier = modifier.padding(32.dp), contentAlignment = Alignment.BottomEnd) {
        Column(
            verticalArrangement = Arrangement.spacedBy(10.dp),
            horizontalAlignment = Alignment.End,
        ) {
            state.toasts.asReversed().forEach { toast ->
                ClientToast(toast, onDismiss = { state.dismiss(toast.id) })
            }
        }
    }
}

@Composable
private fun ClientToast(toast: VisibleToast, onDismiss: () -> Unit) {
    val accent = when (toast.notification.kind) {
        ClientNotificationKind.SUCCESS -> SwarmGreen
        ClientNotificationKind.WARNING -> Color(0xFFF5C451)
        ClientNotificationKind.ERROR -> Color(0xFFFF5D7A)
    }
    val symbol = when (toast.notification.kind) {
        ClientNotificationKind.SUCCESS -> "✓"
        ClientNotificationKind.WARNING -> "!"
        ClientNotificationKind.ERROR -> "×"
    }
    val duration = when (toast.notification.kind) {
        ClientNotificationKind.SUCCESS -> 4_000L
        ClientNotificationKind.WARNING -> 5_000L
        ClientNotificationKind.ERROR -> 7_000L
    }
    var visible by remember(toast.id) { mutableStateOf(false) }
    LaunchedEffect(toast.id) {
        visible = true
        delay(duration)
        visible = false
        delay(180)
        onDismiss()
    }

    AnimatedVisibility(
        visible = visible,
        enter = fadeIn() + slideInHorizontally { it / 3 },
        exit = fadeOut() + slideOutHorizontally { it / 3 },
    ) {
        val shape = RoundedCornerShape(10.dp)
        Row(
            modifier = Modifier
                .height(IntrinsicSize.Min)
                .widthIn(min = 300.dp, max = 460.dp)
                .shadow(12.dp, shape)
                .clip(shape)
                .background(SwarmSurface)
                .border(1.dp, SwarmBorder, shape),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.width(5.dp).fillMaxHeight().background(accent))
            Text(
                symbol,
                color = accent,
                fontSize = 24.sp,
                fontWeight = FontWeight.Black,
                modifier = Modifier.padding(start = 14.dp, end = 10.dp),
            )
            Text(
                toast.notification.message,
                color = SwarmText,
                fontSize = 14.sp,
                maxLines = 4,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(top = 14.dp, end = 16.dp, bottom = 14.dp),
            )
        }
    }
}
