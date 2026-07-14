package com.agentweave.mobile

import android.content.Context
import android.os.Bundle
import android.Manifest
import android.os.Build
import androidx.activity.SystemBarStyle
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.ComponentActivity
import androidx.activity.enableEdgeToEdge
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewmodel.CreationExtras
import com.agentweave.mobile.runtime.RuntimeBridge
import com.agentweave.mobile.runtime.RuntimeClient
import com.agentweave.mobile.runtime.AndroidAgentAppAppearanceStore
import com.agentweave.mobile.runtime.AndroidAgentAppLocalizationStore
import com.agentweave.mobile.secrets.AndroidKeystoreModelSecretStore
import com.agentweave.mobile.ui.AppRoot
import com.agentweave.mobile.ui.AgentWeaveTheme
import com.agentweave.mobile.ui.LocalAppStrings
import com.agentweave.mobile.ui.RuntimeTurnGate
import com.agentweave.mobile.ui.RuntimeSettingsGate

class MainActivity : ComponentActivity() {
  private val requestNotifications =
    registerForActivityResult(ActivityResultContracts.RequestPermission()) { }

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    enableEdgeToEdge()
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
      requestNotifications.launch(Manifest.permission.POST_NOTIFICATIONS)
    }
    val runtimeViewModel = ViewModelProvider(
      this,
      RuntimeClientViewModel.factory { RuntimeDependencies.runtimeLoader(this) },
    )[RuntimeClientViewModel::class.java]
    val client = runtimeViewModel.client
    val diagnostics = client.diagnostics()
    val secretStore = AndroidKeystoreModelSecretStore(this)
    val appearanceStore = AndroidAgentAppAppearanceStore(this)
    val appearance = appearanceStore.appearance
    val localizationStore = AndroidAgentAppLocalizationStore(this)
    val localization = localizationStore.localization
    setContent {
      var selectedThemeId by remember { mutableStateOf(appearanceStore.selectedTheme()) }
      var selectedLocaleId by remember { mutableStateOf(localizationStore.selectedLocale()) }
      CompositionLocalProvider(LocalAppStrings provides localization.strings(selectedLocaleId)) {
        AgentWeaveTheme(appearance, selectedThemeId) {
        val selectedTheme = appearance.themes.firstOrNull { it.id == selectedThemeId }
        val lightTheme = selectedTheme?.type == "light" || selectedTheme?.type == "hcLight"
        SideEffect {
          val systemBarStyle = if (lightTheme) {
            SystemBarStyle.light(android.graphics.Color.TRANSPARENT, android.graphics.Color.TRANSPARENT)
          } else {
            SystemBarStyle.dark(android.graphics.Color.TRANSPARENT)
          }
          enableEdgeToEdge(
            statusBarStyle = systemBarStyle,
            navigationBarStyle = systemBarStyle,
          )
        }
        AppRoot(
          runtimeClient = client,
          turnGate = runtimeViewModel.turnGate,
          settingsGate = runtimeViewModel.settingsGate,
          initialDiagnostics = diagnostics,
          secretStore = secretStore,
          appearance = appearance,
          selectedThemeId = selectedThemeId,
          localization = localization,
          selectedLocaleId = selectedLocaleId,
          onThemeSelected = { themeId ->
            appearanceStore.selectTheme(themeId)
            selectedThemeId = appearance.admittedTheme(themeId)
          },
          onLocaleSelected = { localeId ->
            localizationStore.selectLocale(localeId)
            selectedLocaleId = localization.admittedLocale(localeId)
          },
          modifier = Modifier
            .background(MaterialTheme.colorScheme.background)
            .safeDrawingPadding(),
        )
        }
      }
    }
  }
}

private class RuntimeClientViewModel(
  val client: RuntimeClient,
  val turnGate: RuntimeTurnGate = RuntimeTurnGate(),
  val settingsGate: RuntimeSettingsGate = RuntimeSettingsGate(),
) : ViewModel() {
  override fun onCleared() {
    turnGate.close()
    settingsGate.close()
    client.close()
  }

  companion object {
    fun factory(load: () -> RuntimeClient): ViewModelProvider.Factory =
      object : ViewModelProvider.Factory {
        override fun <T : ViewModel> create(modelClass: Class<T>, extras: CreationExtras): T {
          require(modelClass == RuntimeClientViewModel::class.java) {
            "unsupported ViewModel: ${modelClass.name}"
          }
          @Suppress("UNCHECKED_CAST")
          return RuntimeClientViewModel(load()) as T
        }
      }
  }
}

internal object RuntimeDependencies {
  private val defaultLoader: (Context) -> RuntimeClient = { context -> RuntimeBridge(context).load() }

  var runtimeLoader: (Context) -> RuntimeClient = defaultLoader

  fun reset() {
    runtimeLoader = defaultLoader
  }
}
