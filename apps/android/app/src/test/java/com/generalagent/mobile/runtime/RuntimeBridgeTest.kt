package com.generalagent.mobile.runtime

import android.content.Context
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class RuntimeBridgeTest {
  @Test
  fun loadInitializesNativeRuntimeWithAppPrivatePathsAndCapabilities() {
    val context = RuntimeEnvironment.getApplication() as Context
    val native = RecordingNativeRuntime()

    val client = RuntimeBridge(context, native).load()
    val request = JSONObject(native.initializeRequest)

    assertEquals(41L, client.handle)
    assertEquals(context.filesDir.absolutePath, request.getString("app_data_dir"))
    assertEquals(context.cacheDir.absolutePath, request.getString("cache_dir"))
    assertTrue(request.getString("database_path").startsWith(context.filesDir.absolutePath))
    assertEquals("android", request.getString("platform"))
    assertEquals(4, request.getJSONArray("capabilities").length())
    assertFalse(native.initializeRequest.contains("api_key", ignoreCase = true))
  }
}

private class RecordingNativeRuntime : NativeRuntimeApi {
  var initializeRequest: String = ""

  override fun initialize(requestJson: String): String {
    initializeRequest = requestJson
    return """{"ok":true,"data":{"handle":41}}"""
  }

  override fun invoke(handle: Long, requestJson: String): String =
    """{"ok":true,"data":null}"""

  override fun sendMessage(handle: Long, requestJson: String, apiKey: String?): String =
    """{"ok":true,"data":{"assistant_text":"ok"}}"""

  override fun close(handle: Long): String = """{"ok":true,"data":null}"""
}
