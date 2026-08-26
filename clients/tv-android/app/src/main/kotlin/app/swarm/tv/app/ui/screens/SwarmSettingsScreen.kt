/** Client configuration for this TV. SWARM membership is managed directly
 * from [SwarmDashboardScreen], where users already see connected servers. */
package app.swarm.tv.app.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.OutlinedTextFieldDefaults
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
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import app.swarm.tv.app.data.KidModeSettings
import app.swarm.tv.app.data.RatingScale
import app.swarm.tv.app.data.ResolvedProblemNotification
import app.swarm.tv.app.ui.components.NumberPadEntry
import app.swarm.tv.app.ui.components.SelectableChip
import app.swarm.tv.app.ui.components.TvOutlinedTextField
import app.swarm.tv.app.ui.components.swarmActionButtonColors
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmError
import app.swarm.tv.app.ui.theme.SwarmBorder
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmSurface
import app.swarm.tv.app.ui.theme.SwarmText
import app.swarm.tv.core.peer.MediaKind
import kotlinx.coroutines.delay
import java.text.DateFormat
import java.util.Date

@Composable
fun SwarmSettingsScreen(
    baseUrl: String,
    deviceName: String,
    busy: Boolean,
    errorMessage: String?,
    onUpdateBaseUrl: (baseUrl: String) -> Unit,
    onUpdateDeviceName: (name: String) -> Unit,
    onBack: () -> Unit,
    kidModeSettings: KidModeSettings?,
    availableGenres: List<String>,
    onEnableKidMode: (pin: String, allowedKinds: Set<MediaKind>, allowedGenres: Set<String>?, maxMovieRating: String?, maxTvRating: String?) -> Unit,
    onUpdateKidModeRules: (allowedKinds: Set<MediaKind>, allowedGenres: Set<String>?, maxMovieRating: String?, maxTvRating: String?) -> Unit,
    onDisableKidMode: () -> Unit,
    notifications: List<ResolvedProblemNotification>,
    onDismissNotification: (ResolvedProblemNotification) -> Unit,
) {
    var baseUrlField by remember(baseUrl) { mutableStateOf(baseUrl) }
    var deviceNameField by remember(deviceName) { mutableStateOf(deviceName) }
    var section by remember { mutableStateOf(SettingsSection.GENERAL) }

    // Same reasoning as SwarmDashboardScreen: reached by a UiState swap,
    // not real navigation, so explicitly seed focus on the first section.
    val firstSectionFocusRequester = remember { FocusRequester() }
    LaunchedEffect(Unit) { firstSectionFocusRequester.requestFocus() }

    // Physical Back used to be a no-op here (no BackHandler was ever wired
    // up), so it fell through to Android's default behavior and killed the
    // whole Activity instead of returning to the dashboard — confirmed as
    // the exact bug the on-screen "Back" button below was silently
    // papering over for remote users. Wiring the real Back button to the
    // same `onBack` the button called is what actually fixes it; the
    // on-screen button/title row is dropped entirely below, both because
    // it's now redundant and to win back the vertical space it took.
    BackHandler(onBack = onBack)

    Column(modifier = Modifier.fillMaxSize().padding(32.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            SettingsTab(
                label = "General",
                selected = section == SettingsSection.GENERAL,
                onClick = { section = SettingsSection.GENERAL },
                modifier = Modifier.focusRequester(firstSectionFocusRequester),
            )
            SettingsTab("Family", section == SettingsSection.FAMILY, { section = SettingsSection.FAMILY })
            SettingsTab(
                if (notifications.isEmpty()) "Notifications" else "Notifications (${notifications.size})",
                section == SettingsSection.NOTIFICATIONS,
                { section = SettingsSection.NOTIFICATIONS },
            )
        }
        Spacer(Modifier.height(16.dp))

        // SettingsPanel below gets weight(1f), not just fillMaxWidth, so its
        // own card background/border fill the rest of the screen below the
        // tab row instead of shrink-wrapping to whatever the shortest tab's
        // content needs and leaving bare background underneath — that dead
        // gap below a small floating card was the real complaint, not
        // spacing inside any one tab. Scrolling moves to SettingsPanel's own
        // inner content Column instead of wrapping this whole Column, since
        // this Column can only give a weighted child a *bounded* height (what
        // weight(1f) needs) by not itself being wrapped in verticalScroll,
        // which unbounds it.
        errorMessage?.let {
            Text(it, color = SwarmError, fontSize = 14.sp)
            Spacer(Modifier.height(10.dp))
        }
        when (section) {
            SettingsSection.GENERAL -> SettingsPanel(
                modifier = Modifier.fillMaxWidth().weight(1f),
            ) {
                Text("Connection", color = SwarmText, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                Spacer(Modifier.height(12.dp))
                SettingFieldRow(
                    value = baseUrlField,
                    onValueChange = { baseUrlField = it },
                    label = "SWARM server URL",
                    saveEnabled = !busy && baseUrlField.isNotBlank() && baseUrlField != baseUrl,
                    onSave = { onUpdateBaseUrl(baseUrlField) },
                )
                Spacer(Modifier.height(10.dp))
                SettingFieldRow(
                    value = deviceNameField,
                    onValueChange = { deviceNameField = it },
                    label = "Device name",
                    saveEnabled = !busy && deviceNameField.isNotBlank() && deviceNameField != deviceName,
                    onSave = { onUpdateDeviceName(deviceNameField) },
                )
                Text(
                    "The device name is stored on this TV; it does not rename an existing SWARM server roster entry.",
                    color = SwarmMuted,
                    fontSize = 11.sp,
                )

            }

            SettingsSection.FAMILY -> SettingsPanel(
                modifier = Modifier.fillMaxWidth().weight(1f),
            ) {
                KidModeCard(
                    kidModeSettings = kidModeSettings,
                    availableGenres = availableGenres,
                    onEnable = onEnableKidMode,
                    onUpdateRules = onUpdateKidModeRules,
                    onDisable = onDisableKidMode,
                )
            }

            SettingsSection.NOTIFICATIONS -> SettingsPanel(
                modifier = Modifier.fillMaxWidth().weight(1f),
            ) {
                NotificationInbox(
                    notifications = notifications,
                    onDismiss = onDismissNotification,
                )
            }
        }
    }
}

private enum class SettingsSection { GENERAL, FAMILY, NOTIFICATIONS }

@Composable
private fun NotificationInbox(
    notifications: List<ResolvedProblemNotification>,
    onDismiss: (ResolvedProblemNotification) -> Unit,
) {
    var selectedKey by remember { mutableStateOf<String?>(null) }
    val selected = notifications.firstOrNull { it.key == selectedKey }
    LaunchedEffect(notifications, selectedKey) {
        if (selectedKey != null && selected == null) selectedKey = null
    }

    Text("Resolved problems", color = SwarmText, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
    Spacer(Modifier.height(6.dp))
    Text(
        "Select a notification to view its resolution, then dismiss it when you are done.",
        color = SwarmMuted,
        fontSize = 12.sp,
    )
    Spacer(Modifier.height(16.dp))
    if (notifications.isEmpty()) {
        Text("No notifications.", color = SwarmMuted, fontSize = 14.sp)
        return
    }

    notifications.forEach { notification ->
        Button(
            onClick = { selectedKey = notification.key },
            modifier = Modifier.fillMaxWidth(),
            colors = swarmActionButtonColors(),
        ) {
            Column(Modifier.fillMaxWidth()) {
                Text(notification.assetTitle ?: "Reported problem resolved", fontWeight = FontWeight.SemiBold)
                Text(
                    "${notification.serverName} • ${DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT).format(Date(notification.resolvedAtMs))}",
                    fontSize = 11.sp,
                    color = SwarmMuted,
                )
            }
        }
        Spacer(Modifier.height(8.dp))
    }

    selected?.let { notification ->
        Spacer(Modifier.height(8.dp))
        Text(notification.assetTitle ?: "Reported problem resolved", color = SwarmText, fontSize = 18.sp, fontWeight = FontWeight.Bold)
        Spacer(Modifier.height(8.dp))
        Text(
            notification.comments?.takeIf { it.isNotBlank() } ?: "The media server marked this problem as resolved.",
            color = SwarmText,
            fontSize = 14.sp,
        )
        Spacer(Modifier.height(12.dp))
        Text("Original report", color = SwarmMuted, fontSize = 11.sp, fontWeight = FontWeight.SemiBold)
        Text(notification.originalMessage, color = SwarmMuted, fontSize = 12.sp)
        Spacer(Modifier.height(16.dp))
        Button(
            onClick = {
                selectedKey = null
                onDismiss(notification)
            },
            colors = swarmActionButtonColors(),
        ) {
            Text("Dismiss notification")
        }
    }
}

@Composable
private fun SettingsTab(label: String, selected: Boolean, onClick: () -> Unit, modifier: Modifier = Modifier) {
    Button(
        onClick = onClick,
        modifier = modifier,
        colors = swarmActionButtonColors(),
    ) {
        Text(label, fontWeight = if (selected) FontWeight.Bold else FontWeight.Medium)
    }
}

@Composable
private fun SettingsPanel(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    // The card frame itself takes the caller's full fillMaxWidth().weight(1f)
    // sizing so its background/border reach the bottom of the available pane
    // regardless of how tall any one tab's content is — content then scrolls
    // inside that fixed-height frame (only kicks in if it's ever taller than
    // the pane) rather than the frame shrink-wrapping to content height and
    // leaving bare, un-carded background below it.
    Column(
        modifier = modifier.clip(RoundedCornerShape(16.dp)).background(SwarmSurface).padding(24.dp),
    ) {
        Column(modifier = Modifier.fillMaxWidth().weight(1f).verticalScroll(rememberScrollState())) {
            content()
        }
    }
}

@Composable
private fun SettingFieldRow(
    value: String,
    onValueChange: (String) -> Unit,
    label: String,
    saveEnabled: Boolean,
    onSave: () -> Unit,
) {
    Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        TvOutlinedTextField(
            value = value,
            onValueChange = onValueChange,
            label = { Text(label) },
            colors = fieldColors(),
            modifier = Modifier.width(520.dp),
        )
        Button(onClick = onSave, enabled = saveEnabled, colors = swarmActionButtonColors()) { Text("Save") }
    }
}

// --- Kid Mode -----------------------------------------------------------

/**
 * A small state machine local to this card, not [SwarmViewModel]: every
 * step here (typing a PIN, drafting rules before Save) is transient
 * editing-in-progress UI state with no reason to survive leaving this
 * screen, the same reasoning `searchText`/`showFilterOverlay` in
 * [CatalogScreen] stay local `remember`ed state instead of living in the
 * ViewModel.
 */
private enum class KidModeStep {
    /** Nothing being edited — just the current on/off status and one button. */
    COLLAPSED,
    /** Already enabled; re-entering the PIN is required before rules become visible/editable. */
    ENTER_PIN,
    /** First-ever setup: choosing a new PIN. */
    SET_PIN,
    /** First-ever setup: re-entering the just-chosen PIN to catch typos before it's saved. */
    CONFIRM_PIN,
    /** PIN already verified (or being set for the first time) — kind/genre/rating rules are visible and editable. */
    EDIT_RULES,
}

private const val KID_MODE_PIN_LENGTH = 4

@Composable
private fun KidModeCard(
    kidModeSettings: KidModeSettings?,
    availableGenres: List<String>,
    onEnable: (pin: String, allowedKinds: Set<MediaKind>, allowedGenres: Set<String>?, maxMovieRating: String?, maxTvRating: String?) -> Unit,
    onUpdateRules: (allowedKinds: Set<MediaKind>, allowedGenres: Set<String>?, maxMovieRating: String?, maxTvRating: String?) -> Unit,
    onDisable: () -> Unit,
) {
    val isEnabled = kidModeSettings?.enabled == true
    var step by remember { mutableStateOf(KidModeStep.COLLAPSED) }
    var pinField by remember(step) { mutableStateOf("") }
    var pinError by remember { mutableStateOf(false) }
    // Only meaningful mid-setup, between SET_PIN and CONFIRM_PIN.
    var pendingNewPin by remember { mutableStateOf<String?>(null) }

    // Draft rules, seeded from the current settings (or sensible "allow
    // everything" defaults for a first-ever setup) whenever rules actually
    // become editable — not on every recomposition, which would stomp
    // in-progress edits.
    var draftKinds by remember { mutableStateOf(setOf(MediaKind.MOVIE, MediaKind.EPISODE, MediaKind.TRACK)) }
    var draftGenres by remember { mutableStateOf<Set<String>?>(null) }
    var draftMaxMovieRating by remember { mutableStateOf<String?>(null) }
    var draftMaxTvRating by remember { mutableStateOf<String?>(null) }
    fun seedDraftFromCurrent() {
        draftKinds = kidModeSettings?.allowedKinds?.takeIf { it.isNotEmpty() } ?: setOf(MediaKind.MOVIE, MediaKind.EPISODE, MediaKind.TRACK)
        draftGenres = kidModeSettings?.allowedGenres
        draftMaxMovieRating = kidModeSettings?.maxMovieRating
        draftMaxTvRating = kidModeSettings?.maxTvRating
    }

    // Let Compose paint the final digit before advancing. Performing the
    // transition inside the keypad's click callback replaced this screen in
    // the same frame, so the fourth slot was never visibly filled even though
    // the correct PIN was processed.
    LaunchedEffect(step, pinField) {
        if (pinField.length != KID_MODE_PIN_LENGTH) return@LaunchedEffect
        delay(100)
        when (step) {
            KidModeStep.ENTER_PIN -> {
                if (kidModeSettings?.pinMatches(pinField) == true) {
                    seedDraftFromCurrent()
                    step = KidModeStep.EDIT_RULES
                } else {
                    pinError = true
                    pinField = ""
                }
            }
            KidModeStep.SET_PIN -> {
                pendingNewPin = pinField
                seedDraftFromCurrent()
                step = KidModeStep.CONFIRM_PIN
            }
            KidModeStep.CONFIRM_PIN -> {
                if (pinField == pendingNewPin) {
                    step = KidModeStep.EDIT_RULES
                } else {
                    pinError = true
                    pinField = ""
                    pendingNewPin = null
                    step = KidModeStep.SET_PIN
                }
            }
            else -> Unit
        }
    }

    Text("Kid Mode", color = SwarmMuted, fontSize = 14.sp)
    Spacer(Modifier.height(12.dp))

    when (step) {
        KidModeStep.COLLAPSED -> {
            Text(
                if (isEnabled) "On — restricting what's shown across this app." else "Off — the full library is browsable.",
                color = SwarmText,
                fontSize = 14.sp,
            )
            Spacer(Modifier.height(10.dp))
            Button(
                onClick = {
                    if (isEnabled) {
                        step = KidModeStep.ENTER_PIN
                    } else {
                        pendingNewPin = null
                        step = KidModeStep.SET_PIN
                    }
                },
                colors = swarmActionButtonColors(),
            ) {
                Text(if (isEnabled) "Manage Kid Mode" else "Turn on Kid Mode")
            }
        }
        KidModeStep.ENTER_PIN -> {
            Text("Enter the PIN to manage Kid Mode", color = SwarmText, fontSize = 14.sp)
            Spacer(Modifier.height(12.dp))
            NumberPadEntry(
                value = pinField,
                maxLength = KID_MODE_PIN_LENGTH,
                onValueChange = { entered ->
                    pinField = entered
                    pinError = false
                },
            )
            if (pinError) {
                Spacer(Modifier.height(10.dp))
                Text("Wrong PIN.", color = SwarmError, fontSize = 13.sp)
            }
        }
        KidModeStep.SET_PIN -> {
            Text("Choose a $KID_MODE_PIN_LENGTH-digit PIN", color = SwarmText, fontSize = 14.sp)
            Spacer(Modifier.height(12.dp))
            NumberPadEntry(
                value = pinField,
                maxLength = KID_MODE_PIN_LENGTH,
                onValueChange = { entered ->
                    pinField = entered
                    pinError = false
                },
            )
        }
        KidModeStep.CONFIRM_PIN -> {
            Text("Confirm the PIN", color = SwarmText, fontSize = 14.sp)
            Spacer(Modifier.height(12.dp))
            NumberPadEntry(
                value = pinField,
                maxLength = KID_MODE_PIN_LENGTH,
                onValueChange = { entered ->
                    pinField = entered
                    pinError = false
                },
            )
            if (pinError) {
                Spacer(Modifier.height(10.dp))
                Text("Didn't match — start over.", color = SwarmError, fontSize = 13.sp)
            }
        }
        KidModeStep.EDIT_RULES -> {
            KidModeRulesEditor(
                availableGenres = availableGenres,
                allowedKinds = draftKinds,
                onToggleKind = { kind -> draftKinds = if (kind in draftKinds) draftKinds - kind else draftKinds + kind },
                allowedGenres = draftGenres,
                onToggleGenre = { genre ->
                    draftGenres = when {
                        draftGenres == null -> setOf(genre)
                        genre in draftGenres!! && draftGenres!!.size == 1 -> null
                        genre in draftGenres!! -> draftGenres!! - genre
                        else -> draftGenres!! + genre
                    }
                },
                maxMovieRating = draftMaxMovieRating,
                onSelectMaxMovieRating = { draftMaxMovieRating = it },
                maxTvRating = draftMaxTvRating,
                onSelectMaxTvRating = { draftMaxTvRating = it },
            )
            Spacer(Modifier.height(16.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Button(
                    onClick = {
                        val pin = pendingNewPin
                        if (pin != null) {
                            onEnable(pin, draftKinds, draftGenres, draftMaxMovieRating, draftMaxTvRating)
                        } else {
                            onUpdateRules(draftKinds, draftGenres, draftMaxMovieRating, draftMaxTvRating)
                        }
                        pendingNewPin = null
                        step = KidModeStep.COLLAPSED
                    },
                    enabled = draftKinds.isNotEmpty(),
                    colors = swarmActionButtonColors(),
                ) {
                    Text(if (pendingNewPin != null) "Turn on Kid Mode" else "Save changes", fontWeight = FontWeight.Bold)
                }
                if (isEnabled) {
                    Button(
                        onClick = { onDisable(); step = KidModeStep.COLLAPSED },
                        colors = swarmActionButtonColors(),
                    ) {
                        Text("Turn off Kid Mode")
                    }
                }
            }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun KidModeRulesEditor(
    availableGenres: List<String>,
    allowedKinds: Set<MediaKind>,
    onToggleKind: (MediaKind) -> Unit,
    allowedGenres: Set<String>?,
    onToggleGenre: (String) -> Unit,
    maxMovieRating: String?,
    onSelectMaxMovieRating: (String?) -> Unit,
    maxTvRating: String?,
    onSelectMaxTvRating: (String?) -> Unit,
) {
    Text("Visible media types", color = SwarmMuted, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
    Spacer(Modifier.height(8.dp))
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        SelectableChip("Movies", isSelected = MediaKind.MOVIE in allowedKinds, onClick = { onToggleKind(MediaKind.MOVIE) })
        SelectableChip("Shows", isSelected = MediaKind.EPISODE in allowedKinds, onClick = { onToggleKind(MediaKind.EPISODE) })
        SelectableChip("Music", isSelected = MediaKind.TRACK in allowedKinds, onClick = { onToggleKind(MediaKind.TRACK) })
    }
    if (allowedKinds.isEmpty()) {
        Spacer(Modifier.height(6.dp))
        Text("At least one type must stay allowed.", color = SwarmError, fontSize = 12.sp)
    }

    if (availableGenres.isNotEmpty()) {
        Spacer(Modifier.height(16.dp))
        Text("Genres (tap to restrict — none selected means all genres)", color = SwarmMuted, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
        Spacer(Modifier.height(8.dp))
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            for (genre in availableGenres) {
                SelectableChip(genre, isSelected = allowedGenres?.contains(genre) == true, onClick = { onToggleGenre(genre) })
            }
        }
    }

    if (MediaKind.MOVIE in allowedKinds) {
        Spacer(Modifier.height(16.dp))
        Text("Max movie rating", color = SwarmMuted, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
        Spacer(Modifier.height(8.dp))
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            SelectableChip("No limit", isSelected = maxMovieRating == null, onClick = { onSelectMaxMovieRating(null) })
            for (rating in RatingScale.MOVIE_ORDER) {
                SelectableChip(rating, isSelected = rating == maxMovieRating, onClick = { onSelectMaxMovieRating(rating) })
            }
        }
        if (maxMovieRating != null) {
            Spacer(Modifier.height(6.dp))
            Text("Movies without a known US rating are hidden while a limit is active.", color = SwarmMuted, fontSize = 11.sp)
        }
    }

    if (MediaKind.EPISODE in allowedKinds) {
        Spacer(Modifier.height(16.dp))
        Text("Max show rating", color = SwarmMuted, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
        Spacer(Modifier.height(8.dp))
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            SelectableChip("No limit", isSelected = maxTvRating == null, onClick = { onSelectMaxTvRating(null) })
            for (rating in RatingScale.TV_ORDER) {
                SelectableChip(rating, isSelected = rating == maxTvRating, onClick = { onSelectMaxTvRating(rating) })
            }
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
