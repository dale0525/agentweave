package com.generalagent.mobile

import android.content.Context
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.ui.Modifier
import com.generalagent.mobile.runtime.RuntimeBridge
import com.generalagent.mobile.runtime.RuntimeClient

class MainActivity : ComponentActivity() {
  private var runtimeClient: RuntimeClient? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    val client = RuntimeDependencies.runtimeLoader(this)
    runtimeClient = client
    val diagnostics = client.diagnostics()
    setContent {
      MaterialTheme {
        Surface(modifier = Modifier.fillMaxSize()) {
          Text("GeneralAgent ${diagnostics.platform}")
        }
      }
    }
  }

  override fun onDestroy() {
    runtimeClient?.close()
    runtimeClient = null
    super.onDestroy()
  }
}

internal object RuntimeDependencies {
  private val defaultLoader: (Context) -> RuntimeClient = { context -> RuntimeBridge(context).load() }

  var runtimeLoader: (Context) -> RuntimeClient = defaultLoader

  fun reset() {
    runtimeLoader = defaultLoader
  }
}
