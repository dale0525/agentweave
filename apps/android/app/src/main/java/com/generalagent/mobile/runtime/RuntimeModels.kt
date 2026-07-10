package com.generalagent.mobile.runtime

data class RuntimeInitRequest(
  val appDataDir: String,
  val cacheDir: String,
  val databasePath: String,
  val skillsDir: String,
  val platform: String = "android",
  val capabilities: List<String> = androidMvpCapabilities(),
)

data class RuntimeDiagnostics(
  val platform: String,
  val capabilities: List<String>,
  val databaseReady: Boolean,
  val skillsReady: Boolean,
  val modelConfigured: Boolean,
)

data class RuntimeSession(
  val id: String,
  val title: String,
  val createdAt: String,
  val updatedAt: String,
)

data class RuntimeMessage(
  val id: String,
  val sessionId: String,
  val role: String,
  val content: String,
  val createdAt: String,
)

data class RuntimeModelConfig(
  val providerId: String,
  val providerName: String,
  val endpointType: String,
  val baseUrl: String,
  val modelName: String,
  val secretId: String?,
  val headers: Map<String, String> = emptyMap(),
)

data class RuntimeTurn(val assistantText: String)
