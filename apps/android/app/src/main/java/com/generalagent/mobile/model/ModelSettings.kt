package com.generalagent.mobile.model

import com.generalagent.mobile.runtime.RuntimeModelConfig

data class ModelSettings(
  val providerId: String,
  val providerName: String,
  val endpointType: String,
  val baseUrl: String,
  val modelName: String,
  val secretId: String?,
  val apiKey: String?,
) {
  fun redactedForRust(): RuntimeModelConfig =
    RuntimeModelConfig(
      providerId = providerId,
      providerName = providerName,
      endpointType = endpointType,
      baseUrl = baseUrl,
      modelName = modelName,
      secretId = secretId,
    )
}
