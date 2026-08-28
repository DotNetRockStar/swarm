/**
 * A D-pad-navigable numeric keypad plus its digit-slot display row — every
 * numeric-code entry in this app (currently the 4-digit Kid Mode PIN) uses
 * this same widget rather than the
 * system keyboard, per this app's Fire TV UX convention: no on-screen text
 * entry needs anything more exotic than a D-pad-navigable number grid.
 * Extracted here once a third consumer (Kid Mode) needed the identical
 * copy-pasted DigitSlot/NumberPad pair the first two screens each carried
 * their own private copy of — this codebase's own point to stop duplicating
 * it.
 */
package app.swarm.tv.app.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.ButtonDefaults
import app.swarm.tv.app.ui.UatTestTags
import app.swarm.tv.app.ui.theme.SwarmSurface
import app.swarm.tv.app.ui.theme.SwarmSurfaceMuted
import app.swarm.tv.app.ui.theme.SwarmText

@Composable
fun NumberPadEntry(
    value: String,
    maxLength: Int,
    onValueChange: (String) -> Unit,
    enabled: Boolean = true,
    modifier: Modifier = Modifier,
    firstKeyFocusRequester: FocusRequester? = null,
) {
    Column(modifier = modifier, horizontalAlignment = Alignment.CenterHorizontally) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            for (i in 0 until maxLength) DigitSlot(value.getOrNull(i))
        }
        Spacer(Modifier.height(12.dp))
        NumberPad(
            enabled = enabled,
            onDigit = { d -> if (value.length < maxLength) onValueChange(value + d) },
            onBackspace = { if (value.isNotEmpty()) onValueChange(value.dropLast(1)) },
            firstKeyFocusRequester = firstKeyFocusRequester,
        )
    }
}

@Composable
private fun DigitSlot(digit: Char?) {
    Box(
        modifier = Modifier
            .size(30.dp, 39.dp)
            .clip(RoundedCornerShape(6.dp))
            .background(if (digit != null) SwarmSurfaceMuted else SwarmSurface),
        contentAlignment = Alignment.Center,
    ) {
        Text(digit?.toString() ?: "", color = SwarmText, fontSize = 18.sp, fontWeight = FontWeight.Bold)
    }
}

private val padRows = listOf(
    listOf('1', '2', '3'),
    listOf('4', '5', '6'),
    listOf('7', '8', '9'),
    listOf(null, '0', '<'),
)

@Composable
private fun NumberPad(enabled: Boolean, onDigit: (Char) -> Unit, onBackspace: () -> Unit, firstKeyFocusRequester: FocusRequester?) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        for (row in padRows) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                for (key in row) {
                    Box(modifier = Modifier.size(48.dp)) {
                        if (key != null) {
                            Button(
                                onClick = { if (key == '<') onBackspace() else onDigit(key) },
                                enabled = enabled,
                                colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText),
                                modifier = Modifier.size(48.dp)
                                    .testTag(UatTestTags.NUMBER_PAD_KEY_PREFIX + if (key == '<') "backspace" else key)
                                    .then(
                                        if (key == '1' && firstKeyFocusRequester != null) Modifier.focusRequester(firstKeyFocusRequester) else Modifier,
                                    ),
                            ) {
                                Text(if (key == '<') "⌫" else key.toString(), fontSize = 17.sp)
                            }
                        }
                    }
                }
            }
        }
    }
}
