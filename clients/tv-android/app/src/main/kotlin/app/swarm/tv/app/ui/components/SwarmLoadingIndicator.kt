/**
 * The mascot-pulse loading GIF plus a randomized cute caption underneath for
 * app startup and catalog loading. Playback intentionally does not use this
 * full-screen treatment: video buffering is reported through the shared
 * toast surface so the last frame remains visible.
 */
package app.swarm.tv.app.ui.components

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.swarm.tv.R
import app.swarm.tv.app.ui.theme.SwarmMuted
import coil.compose.AsyncImage

/** One picked at random each time this composable enters composition — ties the wait back to the mascot/"swarm" theme instead of a bare spinner. */
private val LOADING_MESSAGES = listOf(
    "Buzzing up your stream…",
    "Gathering the hive…",
    "Swarm is on the move…",
    "Waggling through the data…",
    "Pollinating your playlist…",
    "Hive mind assembling…",
    "Bee right there…",
    "Making a beeline for your show…",
    "Warming up the honeycomb…",
    "Swarm intelligence at work…",
    "Fetching a fresh frame…",
    "Teaching pixels to fly…",
    "Calling in the worker bees…",
    "Uncapping the honey jar…",
    "Finding the coziest stream…",
    "Fluffing the movie cushions…",
    "Tuning the hive radio…",
    "Rolling out the tiny red carpet…",
    "Checking the buzz levels…",
    "Asking the queen for directions…",
    "Packing snacks for the stream…",
    "Dusting off the pixels…",
    "Following the honey trail…",
    "Preparing for takeoff…",
    "Rallying the swarm…",
    "Stirring up some movie magic…",
    "Finding your place in the hive…",
    "Putting the buzz in buffering…",
    "Sending scouts for your show…",
    "Counting wings and frames…",
    "Building a tiny cinema…",
    "Lighting up the honeycomb…",
    "Carrying your stream home…",
    "Giving the pixels a pep talk…",
    "Almost ready to bee-gin…",
    "Keeping the hive entertained…",
    "Unlocking the next scene…",
    "Pouring a smooth stream…",
    "Hatching a brilliant picture…",
    "Waking the projection bee…",
    "Making room on the screen…",
    "Putting every bee in formation…",
    "Gathering a cloud of pixels…",
    "Searching the hive archives…",
    "Spinning up the story…",
    "Sweetening your screen…",
    "Guiding the stream to your TV…",
    "Getting the wings in sync…",
    "Opening night at the hive…",
    "One tiny buzz away…",
)

@Composable
fun SwarmLoadingIndicator(
    modifier: Modifier = Modifier,
    messageOverride: String? = null,
) {
    val loadingMessage = remember(messageOverride) { messageOverride ?: LOADING_MESSAGES.random() }
    Column(modifier = modifier, horizontalAlignment = Alignment.CenterHorizontally) {
        AsyncImage(model = R.drawable.loading, contentDescription = null, modifier = Modifier.size(160.dp))
        Spacer(Modifier.height(14.dp))
        Text(loadingMessage, color = SwarmMuted, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
    }
}
