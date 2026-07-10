package com.generalagent.mobile.model

import com.generalagent.mobile.runtime.RuntimeModelConfig
import java.net.URI

data class ModelSettings(
  val providerId: String,
  val providerName: String,
  val endpointType: String,
  val baseUrl: String,
  val modelName: String,
  val secretId: String?,
  val apiKey: String?,
) {
  fun redactedForRust(): RuntimeModelConfig {
    val normalizedBaseUrl = baseUrl.trim()
    val uri = try {
      URI(normalizedBaseUrl)
    } catch (error: Exception) {
      throw IllegalArgumentException("model base URL is invalid", error)
    }
    require(uri.scheme == "https" || uri.scheme == "http") {
      "model base URL must use HTTP or HTTPS"
    }
    require(!uri.host.isNullOrBlank()) { "model base URL host is required" }
    require(uri.userInfo == null) { "model base URL must not contain credentials" }
    require(uri.rawQuery == null) { "model base URL must not contain query parameters" }
    require(uri.rawFragment == null) { "model base URL must not contain a fragment" }

    val normalizedSecretId = secretId?.trim()?.also { reference ->
      require(SECRET_REFERENCE.matches(reference)) {
        "model secret reference must use the model.* namespace"
      }
    }
    if (!apiKey.isNullOrBlank()) {
      require(normalizedSecretId != null) { "model secret reference is required for an API key" }
    }

    return RuntimeModelConfig(
      providerId = providerId,
      providerName = providerName,
      endpointType = endpointType,
      baseUrl = normalizedBaseUrl,
      modelName = modelName,
      secretId = normalizedSecretId,
    )
  }

  private companion object {
    val SECRET_REFERENCE = Regex("^model\\.[A-Za-z0-9][A-Za-z0-9._-]{0,126}$")
  }
}
