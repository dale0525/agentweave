package com.generalagent.mobile.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
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

  @Test
  fun redactedSettingsRejectCredentialsInBaseUrl() {
    val withUserInfo = settings(baseUrl = "https://user:sk-secret@api.example.com/v1")
    val withQuery = settings(baseUrl = "https://api.example.com/v1?api_key=sk-secret")

    assertThrows(IllegalArgumentException::class.java) { withUserInfo.redactedForRust() }
    assertThrows(IllegalArgumentException::class.java) { withQuery.redactedForRust() }
  }

  @Test
  fun redactedSettingsRejectSecretValueAsReference() {
    val settings = settings(secretId = "sk-secret")

    assertThrows(IllegalArgumentException::class.java) { settings.redactedForRust() }
  }

  @Test
  fun redactedSettingsAcceptUppercaseHttpScheme() {
    val redacted = settings(baseUrl = "HTTPS://api.example.com/v1").redactedForRust()

    assertEquals("HTTPS://api.example.com/v1", redacted.baseUrl)
  }

  private fun settings(
    baseUrl: String = "https://api.example.com/v1",
    secretId: String = "model.example.default",
  ): ModelSettings =
    ModelSettings(
      providerId = "example",
      providerName = "Example",
      endpointType = "responses",
      baseUrl = baseUrl,
      modelName = "model",
      secretId = secretId,
      apiKey = "sk-secret",
    )
}
