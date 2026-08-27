/**
 * A single selectable chip — the "tag cloud" pill button used by every
 * FlowRow-based picker in this app (the catalog Filter modal's Show/Genre/
 * Rating/Liked sections, Kid Mode's rules editor). Pulled out once a second
 * near-identical copy of this same button (this codebase's own point to
 * stop duplicating something, same reasoning [NumberPadEntry] was
 * extracted at its own third copy).
 */
package app.swarm.tv.app.ui.components

import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.ButtonDefaults
import app.swarm.tv.app.ui.theme.SwarmAccentHot

@Composable
fun SelectableChip(
    label: String,
    isSelected: Boolean,
    onClick: () -> Unit,
    focusRequester: FocusRequester? = null,
    modifier: Modifier = Modifier,
) {
    val focusModifier = if (focusRequester != null) Modifier.focusRequester(focusRequester) else Modifier
    Button(
        onClick = onClick,
        modifier = modifier.then(focusModifier),
        // Real bug, found live: with only containerColor/contentColor set,
        // this button's *focused* state (what's actually visible almost the
        // entire time a user is navigating this list — that's the point of
        // a picker) fell back to tv-material3's own default focused colors
        // instead of this app's theme, rendering as barely-legible black
        // text on a light gray highlight. Every button elsewhere in this
        // app happens to not linger in a focused state long enough for that
        // gap to be obvious; a picker the user deliberately scans through
        // many chips of is exactly where it shows up. Explicit focused (and
        // pressed, same gap) colors close it for good rather than relying
        // on the library default matching this app's dark theme by luck.
        colors = ButtonDefaults.colors(
            containerColor = if (isSelected) SwarmAccentHot else Color.White,
            contentColor = Color(0xFF04263A),
            focusedContainerColor = SwarmAccentHot,
            focusedContentColor = Color(0xFF04263A),
            pressedContainerColor = SwarmAccentHot,
            pressedContentColor = Color(0xFF04263A),
        ),
    ) {
        Text(label, fontSize = 13.sp, maxLines = 1)
    }
}
