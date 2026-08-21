/**
 * Multi-swarm membership management, reached from [SwarmDashboardScreen]:
 * lists every swarm this device belongs to, lets the user switch which one
 * is active, leave one, or join an additional one with a fresh 8-digit
 * code; also the app's one configuration page — editable STUN server URL
 * and device name (both remembered across launches via
 * [app.swarm.tv.app.data.AndroidConnectionStore], previously entered once
 * at registration and never revisited — see that store's doc comment) and
 * the artwork cache TTL (minutes, [app.swarm.tv.app.ui.ArtworkCache]).
 */
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.ButtonDefaults
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import app.swarm.tv.app.data.KidModeSettings
import app.swarm.tv.app.data.RatingScale
import app.swarm.tv.app.ui.components.NumberPadEntry
import app.swarm.tv.app.ui.components.SelectableChip
import app.swarm.tv.app.ui.components.TvOutlinedTextField
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmAccentHot
import app.swarm.tv.app.ui.theme.SwarmBorder
import app.swarm.tv.app.ui.theme.SwarmGreen
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmSurface
import app.swarm.tv.app.ui.theme.SwarmSurfaceMuted
import app.swarm.tv.app.ui.theme.SwarmText
import app.swarm.tv.core.peer.MediaKind
import app.swarm.tv.core.rest.SwarmSummary

@Composable
fun SwarmSettingsScreen(
    allSwarms: List<SwarmSummary>,
    activeSwarmId: String?,
    baseUrl: String,
    deviceName: String,
    artworkCacheMinutes: Int,
    busy: Boolean,
    errorMessage: String?,
    onJoin: (code: String) -> Unit,
    onLeave: (swarmId: String) -> Unit,
    onSwitchActive: (swarmId: String) -> Unit,
    onUpdateBaseUrl: (baseUrl: String) -> Unit,
    onUpdateDeviceName: (name: String) -> Unit,
    onUpdateArtworkCacheMinutes: (minutes: Int) -> Unit,
    onBack: () -> Unit,
    kidModeSettings: KidModeSettings?,
    availableGenres: List<String>,
    onEnableKidMode: (pin: String, allowedKinds: Set<MediaKind>, allowedGenres: Set<String>?, maxMovieRating: String?, maxTvRating: String?) -> Unit,
    onUpdateKidModeRules: (allowedKinds: Set<MediaKind>, allowedGenres: Set<String>?, maxMovieRating: String?, maxTvRating: String?) -> Unit,
    onDisableKidMode: () -> Unit,
) {
    var code by remember { mutableStateOf("") }
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
            SettingsTab("Swarms", section == SettingsSection.SWARMS, { section = SettingsSection.SWARMS })
            SettingsTab("Family", section == SettingsSection.FAMILY, { section = SettingsSection.FAMILY })
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
            Text(it, color = SwarmAccentHot, fontSize = 14.sp)
            Spacer(Modifier.height(10.dp))
        }
        when (section) {
            SettingsSection.GENERAL -> SettingsPanel(
                title = "General",
                subtitle = "Connection identity and artwork behavior for this TV.",
                modifier = Modifier.fillMaxWidth().weight(1f),
            ) {
                Text("Connection", color = SwarmText, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                Spacer(Modifier.height(12.dp))
                SettingFieldRow(
                    value = baseUrlField,
                    onValueChange = { baseUrlField = it },
                    label = "STUN server URL",
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
                    "The device name is stored on this TV; it does not rename an existing STUN roster entry.",
                    color = SwarmMuted,
                    fontSize = 11.sp,
                )

                Spacer(Modifier.height(24.dp))
                Text("Artwork cache", color = SwarmText, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                Spacer(Modifier.height(10.dp))
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                    Button(
                        onClick = { onUpdateArtworkCacheMinutes(artworkCacheMinutes - 1) },
                        enabled = !busy && artworkCacheMinutes > 0,
                        colors = secondaryButtonColors(),
                    ) { Text("−") }
                    Text(
                        if (artworkCacheMinutes == 1) "1 minute" else "$artworkCacheMinutes minutes",
                        color = SwarmText,
                        fontSize = 16.sp,
                        fontWeight = FontWeight.SemiBold,
                        modifier = Modifier.width(120.dp),
                    )
                    Button(
                        onClick = { onUpdateArtworkCacheMinutes(artworkCacheMinutes + 1) },
                        enabled = !busy && artworkCacheMinutes < 1440,
                        colors = secondaryButtonColors(),
                    ) { Text("+") }
                }
                Spacer(Modifier.height(6.dp))
                Text("0 always re-fetches; up to 1,440 minutes keeps browsing fast on slower networks.", color = SwarmMuted, fontSize = 11.sp)
            }

            SettingsSection.SWARMS -> SettingsPanel(
                title = "Swarms",
                subtitle = "Choose the active swarm, leave one, or join another.",
                modifier = Modifier.fillMaxWidth().weight(1f),
            ) {
                Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(28.dp), verticalAlignment = Alignment.Top) {
                    Column(modifier = Modifier.weight(1f)) {
                        Text("Joined (${allSwarms.size})", color = SwarmText, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                        Spacer(Modifier.height(12.dp))
                        if (allSwarms.isEmpty()) {
                            Text("This device has not joined a swarm yet.", color = SwarmMuted, fontSize = 14.sp)
                        } else {
                            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                                for (swarm in allSwarms) {
                                    SwarmRow(
                                        swarm = swarm,
                                        isActive = swarm.id == activeSwarmId,
                                        busy = busy,
                                        onSelect = { onSwitchActive(swarm.id) },
                                        onLeave = { onLeave(swarm.id) },
                                    )
                                }
                            }
                        }
                    }
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        Text("Join another", color = SwarmText, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                        Spacer(Modifier.height(12.dp))
                        NumberPadEntry(value = code, maxLength = 8, onValueChange = { code = it }, enabled = !busy)
                        Spacer(Modifier.height(14.dp))
                        Button(
                            onClick = { onJoin(code); code = "" },
                            enabled = !busy && code.length == 8,
                            colors = ButtonDefaults.colors(containerColor = SwarmAccent, contentColor = Color(0xFF04263A)),
                        ) {
                            Text(if (busy) "Working…" else "Join swarm", fontWeight = FontWeight.Bold)
                        }
                    }
                }
            }

            SettingsSection.FAMILY -> SettingsPanel(
                title = "Family controls",
                subtitle = "Limit the library shown on this TV with a PIN.",
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
        }
    }
}

private enum class SettingsSection { GENERAL, SWARMS, FAMILY }

@Composable
private fun SettingsTab(label: String, selected: Boolean, onClick: () -> Unit, modifier: Modifier = Modifier) {
    Button(
        onClick = onClick,
        modifier = modifier,
        colors = ButtonDefaults.colors(
            containerColor = if (selected) SwarmAccent else SwarmSurfaceMuted,
            contentColor = if (selected) Color(0xFF04263A) else SwarmText,
        ),
    ) {
        Text(label, fontWeight = if (selected) FontWeight.Bold else FontWeight.Medium)
    }
}

@Composable
private fun SettingsPanel(title: String, subtitle: String, modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    // The card frame itself takes the caller's full fillMaxWidth().weight(1f)
    // sizing so its background/border reach the bottom of the available pane
    // regardless of how tall any one tab's content is — content then scrolls
    // inside that fixed-height frame (only kicks in if it's ever taller than
    // the pane) rather than the frame shrink-wrapping to content height and
    // leaving bare, un-carded background below it.
    Column(
        modifier = modifier.clip(RoundedCornerShape(16.dp)).background(SwarmSurface).padding(24.dp),
    ) {
        Text(title, color = SwarmText, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Text(subtitle, color = SwarmMuted, fontSize = 13.sp)
        Spacer(Modifier.height(20.dp))
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
        Button(onClick = onSave, enabled = saveEnabled, colors = secondaryButtonColors()) { Text("Save") }
    }
}

@Composable
private fun secondaryButtonColors() = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText)

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
    var pinError by remember(step) { mutableStateOf(false) }
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
                colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText),
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
                    if (entered.length == KID_MODE_PIN_LENGTH) {
                        if (kidModeSettings?.pinMatches(entered) == true) {
                            seedDraftFromCurrent()
                            step = KidModeStep.EDIT_RULES
                        } else {
                            pinError = true
                            pinField = ""
                        }
                    }
                },
            )
            if (pinError) {
                Spacer(Modifier.height(10.dp))
                Text("Wrong PIN.", color = SwarmAccentHot, fontSize = 13.sp)
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
                    if (entered.length == KID_MODE_PIN_LENGTH) {
                        pendingNewPin = entered
                        seedDraftFromCurrent()
                        step = KidModeStep.CONFIRM_PIN
                    }
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
                    if (entered.length == KID_MODE_PIN_LENGTH) {
                        if (entered == pendingNewPin) {
                            step = KidModeStep.EDIT_RULES
                        } else {
                            pinError = true
                            pinField = ""
                            pendingNewPin = null
                            step = KidModeStep.SET_PIN
                        }
                    }
                },
            )
            if (pinError) {
                Spacer(Modifier.height(10.dp))
                Text("Didn't match — start over.", color = SwarmAccentHot, fontSize = 13.sp)
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
                    colors = ButtonDefaults.colors(containerColor = SwarmAccent, contentColor = Color(0xFF04263A)),
                ) {
                    Text("Save", fontWeight = FontWeight.Bold)
                }
                if (isEnabled) {
                    Button(
                        onClick = { onDisable(); step = KidModeStep.COLLAPSED },
                        colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmAccentHot),
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
    Text("Allowed", color = SwarmMuted, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
    Spacer(Modifier.height(8.dp))
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        SelectableChip("Movies", isSelected = MediaKind.MOVIE in allowedKinds, onClick = { onToggleKind(MediaKind.MOVIE) })
        SelectableChip("Shows", isSelected = MediaKind.EPISODE in allowedKinds, onClick = { onToggleKind(MediaKind.EPISODE) })
        SelectableChip("Music", isSelected = MediaKind.TRACK in allowedKinds, onClick = { onToggleKind(MediaKind.TRACK) })
    }
    if (allowedKinds.isEmpty()) {
        Spacer(Modifier.height(6.dp))
        Text("At least one type must stay allowed.", color = SwarmAccentHot, fontSize = 12.sp)
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

@Composable
private fun SwarmRow(swarm: SwarmSummary, isActive: Boolean, busy: Boolean, onSelect: () -> Unit, onLeave: () -> Unit) {
    // Two independently-reachable D-pad targets, not one Card wrapping a
    // nested Button — a clickable Card's own focus/click semantics swallow
    // D-pad traversal to children nested inside it, making the inner
    // control unreachable (confirmed on real hardware: RIGHT/UP/DOWN off
    // the Card never focused the nested "Leave" button). Both the select
    // surface and the Leave button are direct children of this Row instead,
    // so LEFT/RIGHT between them works via normal 2D focus search.
    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Card(
            onClick = onSelect,
            modifier = Modifier.weight(1f),
            colors = CardDefaults.colors(containerColor = if (isActive) SwarmSurfaceMuted else SwarmSurface),
        ) {
            Column(modifier = Modifier.fillMaxWidth().padding(16.dp)) {
                Text(swarm.name, color = SwarmText, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                if (isActive) Text("active", color = SwarmGreen, fontSize = 12.sp)
            }
        }
        Button(
            onClick = onLeave,
            enabled = !busy,
            colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmAccentHot),
        ) {
            Text("Leave")
        }
    }
}
