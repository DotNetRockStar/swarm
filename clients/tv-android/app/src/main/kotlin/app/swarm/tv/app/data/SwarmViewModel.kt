package app.swarm.tv.app.data

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import app.swarm.tv.core.client.StunApiClient
import app.swarm.tv.core.client.StunClientError
import app.swarm.tv.core.rest.DeviceRegistration
import app.swarm.tv.core.rest.DeviceType
import app.swarm.tv.core.rest.SwarmDevice
import app.swarm.tv.core.rest.SwarmSummary
import app.swarm.tv.core.token.TokenStore
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

sealed class UiState {
    data object PasscodeEntry : UiState()
    data object Registering : UiState()
    data class Dashboard(val swarm: SwarmSummary, val devices: List<SwarmDevice>, val resyncing: Boolean = false) : UiState()
    data class Error(val message: String) : UiState()
}

/**
 * Onboarding + swarm-dashboard state. Registration itself is fully wired
 * (real network calls against a real STUN server, real encrypted token
 * storage, a real device identity fingerprint); the merged multi-server
 * catalog and P2P playback screens land once the peer QUIC transport does
 * (see `docs/PROTOCOL.md` and the risk register on kwik throughput) — this
 * ViewModel's `Dashboard` state exposes the swarm roster now so that piece
 * has something real to build on.
 */
class SwarmViewModel(
    private val tokenStore: TokenStore,
    private val machineId: String,
    private val certFingerprint: String,
) : ViewModel() {
    private val _state = MutableStateFlow<UiState>(UiState.PasscodeEntry)
    val state: StateFlow<UiState> = _state.asStateFlow()

    private var client: StunApiClient? = null
    private var accessToken: String? = null
    private var swarmId: String? = null

    fun submitPasscode(baseUrl: String, code: String, deviceName: String) {
        val trimmedBaseUrl = baseUrl.trim()
        val trimmedCode = code.trim()
        if (trimmedBaseUrl.isEmpty() || trimmedCode.length != 8) {
            _state.value = UiState.Error("Enter the STUN server URL and the 8-digit join code.")
            return
        }
        viewModelScope.launch {
            _state.value = UiState.Registering
            val api = StunApiClient(trimmedBaseUrl)
            try {
                val response = api.registerDevice(
                    trimmedCode,
                    DeviceRegistration(
                        name = deviceName.ifBlank { "Fire TV" },
                        deviceType = DeviceType.CLIENT,
                        machineId = machineId,
                        certFingerprint = certFingerprint,
                        platform = "android-tv",
                        appVersion = "0.1.0",
                    ),
                )
                tokenStore.save(response.accessToken)
                client = api
                accessToken = response.accessToken
                swarmId = response.swarm.id
                loadRoster()
            } catch (e: StunClientError) {
                _state.value = UiState.Error(e.message ?: "Could not join that swarm.")
            }
        }
    }

    fun resync() {
        val current = _state.value
        if (current !is UiState.Dashboard) return
        _state.value = current.copy(resyncing = true)
        viewModelScope.launch { loadRoster() }
    }

    fun dismissError() {
        _state.value = UiState.PasscodeEntry
    }

    private suspend fun loadRoster() {
        val api = client ?: return
        val token = accessToken ?: return
        val id = swarmId ?: return
        try {
            val roster = api.swarmDevices(token, id)
            _state.value = UiState.Dashboard(roster.swarm, roster.devices)
        } catch (e: StunClientError) {
            _state.value = UiState.Error(e.message ?: "Could not load the swarm roster.")
        }
    }
}
