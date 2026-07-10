package com.generalagent.mobile.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

class ModelSettingsTest {
  @Test
  fun redactedSettingsNeverContainApiKey() {
    val settings =
      ModelSettings(
        providerId = "openai",
        providerName = "OpenAI",
        endpointType = "responses",
        baseUrl = "https://api.openai.com/v1",
        modelName = "gpt-5.4",
        secretId = "model.openai.default",
        apiKey = "sk-secret",
      )

    val redacted = settings.redactedForRust()

    assertEquals("model.openai.default", redacted.secretId)
    assertFalse(redacted.toString().contains("sk-secret"))
  }
}
