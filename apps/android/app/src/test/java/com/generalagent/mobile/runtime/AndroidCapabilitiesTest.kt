package com.generalagent.mobile.runtime

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AndroidCapabilitiesTest {
  @Test
  fun androidMvpCapabilitiesContainOnlyMobileSafeCoreCapabilities() {
    val capabilities = androidMvpCapabilities()

    assertEquals(
      listOf("network.http", "filesystem.app_data", "secure_storage", "model.http_provider"),
      capabilities,
    )
    assertTrue(capabilities.contains("network.http"))
    assertFalse(capabilities.contains("shell.process"))
    assertFalse(capabilities.contains("browser.headless"))
  }
}
