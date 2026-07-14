package com.agentweave.mobile

import com.agentweave.mobile.runtime.NativeRuntimeApi
import com.agentweave.mobile.runtime.RuntimeClient
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class MainActivityTest {
  private val native = ActivityNativeRuntime()

  @After
  fun resetDependencies() {
    RuntimeDependencies.reset()
  }

  @Test
  fun activityInitializesRuntimeClientAndClosesItOnDestroy() {
    RuntimeDependencies.runtimeLoader = { RuntimeClient(7L, native) }

    val controller = Robolectric.buildActivity(MainActivity::class.java).setup()

    assertFalse(controller.get().isFinishing)
    controller.destroy()
    assertTrue(native.closed)
  }

  @Test
  fun activityRetainsRuntimeClientAcrossConfigurationChange() {
    var loadCount = 0
    RuntimeDependencies.runtimeLoader = {
      loadCount += 1
      RuntimeClient(7L, native)
    }

    val controller = Robolectric.buildActivity(MainActivity::class.java).setup()
    controller.configurationChange()

    assertEquals(1, loadCount)
    assertFalse(native.closed)

    controller.destroy()
    assertTrue(native.closed)
  }
}

private class ActivityNativeRuntime : NativeRuntimeApi {
  var closed = false

  override fun initialize(requestJson: String): String = error("not used")

  override fun invoke(handle: Long, requestJson: String): String =
    """{"ok":true,"data":{"platform":"android","capabilities":["network.http"],"database_ready":true,"skills_ready":true,"model_configured":false}}"""

  override fun sendMessage(handle: Long, requestJson: String, apiKey: String?): String = error("not used")

  override fun close(handle: Long): String {
    closed = true
    return """{"ok":true,"data":null}"""
  }
}
