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

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
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

    // Same reasoning as SwarmDashboardScreen: reached by a UiState swap, not
    // real navigation, so nothing else ever moves D-pad focus here.
    val backFocusRequester = remember { FocusRequester() }
    LaunchedEffect(Unit) { backFocusRequester.requestFocus() }

    Column(modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(40.dp)) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("Swarm settings", color = SwarmAccent, fontSize = 22.sp, fontWeight = FontWeight.Black)
            Button(
                onClick = onBack,
                modifier = Modifier.focusRequester(backFocusRequester),
                colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText),
            ) {
                Text("Back")
            }
        }
        Spacer(Modifier.height(24.dp))

        Text("Connection", color = SwarmMuted, fontSize = 14.sp)
        Spacer(Modifier.height(12.dp))
        Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            TvOutlinedTextField(
                value = baseUrlField,
                onValueChange = { baseUrlField = it },
                label = { Text("STUN server URL") },
                colors = fieldColors(),
                modifier = Modifier.width(420.dp),
            )
            Button(
                onClick = { onUpdateBaseUrl(baseUrlField) },
                enabled = !busy && baseUrlField.isNotBlank() && baseUrlField != baseUrl,
                colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText),
            ) {
                Text("Save")
            }
        }
        Spacer(Modifier.height(10.dp))
        Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            TvOutlinedTextField(
                value = deviceNameField,
                onValueChange = { deviceNameField = it },
                label = { Text("Device name") },
                colors = fieldColors(),
                modifier = Modifier.width(420.dp),
            )
            Button(
                onClick = { onUpdateDeviceName(deviceNameField) },
                enabled = !busy && deviceNameField.isNotBlank() && deviceNameField != deviceName,
                colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText),
            ) {
                Text("Save")
            }
        }
        Text(
            "Renaming here only updates what this app remembers locally — it doesn't rename the device other swarm members already see.",
            color = SwarmMuted,
            fontSize = 11.sp,
        )

        Spacer(Modifier.height(28.dp))
        Text("Artwork cache", color = SwarmMuted, fontSize = 14.sp)
        Spacer(Modifier.height(12.dp))
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            Button(
                onClick = { onUpdateArtworkCacheMinutes(artworkCacheMinutes - 1) },
                enabled = !busy && artworkCacheMinutes > 0,
                colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText),
            ) { Text("−") }
            Text(
                if (artworkCacheMinutes == 1) "1 minute" else "$artworkCacheMinutes minutes",
                color = SwarmText,
                fontSize = 16.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier.width(110.dp),
            )
            Button(
                onClick = { onUpdateArtworkCacheMinutes(artworkCacheMinutes + 1) },
                enabled = !busy && artworkCacheMinutes < 1440,
                colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText),
            ) { Text("+") }
        }
        Text(
            "How long a cached poster/cover image is trusted before re-fetching. 0 always re-fetches.",
            color = SwarmMuted,
            fontSize = 11.sp,
        )

        Spacer(Modifier.height(28.dp))
        Text("Joined swarms (${allSwarms.size})", color = SwarmMuted, fontSize = 14.sp)
        Spacer(Modifier.height(12.dp))
        if (allSwarms.isEmpty()) {
            Text("Not a member of any swarm yet — join one below.", color = SwarmMuted, fontSize = 14.sp)
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

        Spacer(Modifier.height(32.dp))
        Text("Join another swarm", color = SwarmMuted, fontSize = 14.sp)
        Spacer(Modifier.height(12.dp))
        NumberPadEntry(value = code, maxLength = 8, onValueChange = { code = it }, enabled = !busy)
        Spacer(Modifier.height(16.dp))
        Button(
            onClick = { onJoin(code); code = "" },
            enabled = !busy && code.length == 8,
            colors = ButtonDefaults.colors(containerColor = SwarmAccent, contentColor = Color(0xFF04263A)),
        ) {
            Text(if (busy) "Working…" else "Join swarm", fontWeight = FontWeight.Bold)
        }

        errorMessage?.let {
            Spacer(Modifier.height(16.dp))
            Text(it, color = SwarmAccentHot, fontSize = 14.sp)
        }

        Spacer(Modifier.height(32.dp))
        KidModeCard(
            kidModeSettings = kidModeSettings,
            availableGenres = availableGenres,
            onEnable = onEnableKidMode,
            onUpdateRules = onUpdateKidModeRules,
            onDisable = onDisableKidMode,
        )
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

