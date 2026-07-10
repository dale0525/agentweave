package com.generalagent.mobile

import android.content.Context
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.enableEdgeToEdge
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.ui.Modifier
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewmodel.CreationExtras
import com.generalagent.mobile.runtime.RuntimeBridge
import com.generalagent.mobile.runtime.RuntimeClient
import com.generalagent.mobile.secrets.AndroidKeystoreModelSecretStore
import com.generalagent.mobile.ui.AppRoot
import com.generalagent.mobile.ui.GeneralAgentTheme
import com.generalagent.mobile.ui.RuntimeTurnGate
import com.generalagent.mobile.ui.RuntimeSettingsGate

class MainActivity : ComponentActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    enableEdgeToEdge()
    val runtimeViewModel = ViewModelProvider(
      this,
      RuntimeClientViewModel.factory { RuntimeDependencies.runtimeLoader(this) },
    )[RuntimeClientViewModel::class.java]
    val client = runtimeViewModel.client
    val diagnostics = client.diagnostics()
    val secretStore = AndroidKeystoreModelSecretStore(this)
    setContent {
      GeneralAgentTheme {
        AppRoot(
          runtimeClient = client,
          turnGate = runtimeViewModel.turnGate,
          settingsGate = runtimeViewModel.settingsGate,
          initialDiagnostics = diagnostics,
          secretStore = secretStore,
          modifier = Modifier.safeDrawingPadding(),
        )
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
