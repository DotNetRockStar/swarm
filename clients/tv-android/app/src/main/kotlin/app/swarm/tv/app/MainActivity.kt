package app.swarm.tv.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewmodel.compose.viewModel
import app.swarm.tv.app.data.AndroidDeviceIdentity
import app.swarm.tv.app.data.AndroidTokenStore
import app.swarm.tv.app.data.SwarmViewModel
import app.swarm.tv.app.data.UiState
import app.swarm.tv.app.data.androidMachineId
import app.swarm.tv.app.ui.screens.PasscodeEntryScreen
import app.swarm.tv.app.ui.screens.SwarmDashboardScreen
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmBackground
import app.swarm.tv.app.ui.theme.SwarmTvTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val tokenStore = AndroidTokenStore(applicationContext)
        val machineId = androidMachineId(applicationContext)
        val certFingerprint = AndroidDeviceIdentity.ensureFingerprint()
        val factory = object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T =
                SwarmViewModel(tokenStore, machineId, certFingerprint) as T
        }

        setContent {
            SwarmTvTheme {
                Box(modifier = Modifier.fillMaxSize().background(SwarmBackground)) {
                    val viewModel: SwarmViewModel = viewModel(factory = factory)
                    val state by viewModel.state.collectAsState()
                    SwarmApp(state, viewModel::submitPasscode, viewModel::resync)
                }
            }
        }
    }
}

@Composable
private fun SwarmApp(
    state: UiState,
    onSubmit: (baseUrl: String, code: String, deviceName: String) -> Unit,
    onResync: () -> Unit,
) {
    when (state) {
        is UiState.PasscodeEntry ->
            PasscodeEntryScreen(isSubmitting = false, errorMessage = null, onSubmit = onSubmit)
        is UiState.Registering ->
            PasscodeEntryScreen(isSubmitting = true, errorMessage = null, onSubmit = onSubmit)
        is UiState.Error ->
            PasscodeEntryScreen(isSubmitting = false, errorMessage = state.message, onSubmit = onSubmit)
        is UiState.Dashboard ->
            SwarmDashboardScreen(state.swarm, state.devices, state.resyncing, onResync)
    }
}
