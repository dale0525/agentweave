package com.generalagent.mobile.runtime

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject

class RuntimeBridge(
  private val context: Context,
  private val native: NativeRuntimeApi = NativeRuntime,
) {
  fun initRequest(): RuntimeInitRequest {
    val filesDir = context.filesDir
    return RuntimeInitRequest(
      appDataDir = filesDir.absolutePath,
      cacheDir = context.cacheDir.absolutePath,
      databasePath = filesDir.resolve("general-agent.db").absolutePath,
      skillsDir = filesDir.resolve("skills").absolutePath,
    )
  }

  fun load(): RuntimeClient {
    val data = responseData(native.initialize(initRequest().toJson().toString()))
    return RuntimeClient(data.getLong("handle"), native)
  }
}

class RuntimeClient internal constructor(
  val handle: Long,
  private val native: NativeRuntimeApi,
) : AutoCloseable {
  fun diagnostics(): RuntimeDiagnostics {
    val data = invoke(JSONObject().put("operation", "diagnostics"))
    return RuntimeDiagnostics(
      platform = data.getString("platform"),
      capabilities = data.getJSONArray("capabilities").strings(),
      databaseReady = data.getBoolean("database_ready"),
      skillsReady = data.getBoolean("skills_ready"),
      modelConfigured = data.getBoolean("model_configured"),
    )
  }

  fun createSession(title: String): RuntimeSession =
    invoke(JSONObject().put("operation", "create_session").put("title", title)).toSession()

  fun listSessions(): List<RuntimeSession> =
    invokeArray(JSONObject().put("operation", "list_sessions")).objects().map { it.toSession() }

  fun getMessages(sessionId: String): List<RuntimeMessage> =
    invokeArray(
      JSONObject().put("operation", "get_messages").put("session_id", sessionId),
    ).objects().map { it.toMessage() }

  fun deleteSession(sessionId: String) {
    invoke(JSONObject().put("operation", "delete_session").put("session_id", sessionId))
  }

  fun saveModelConfig(config: RuntimeModelConfig) {
    invoke(JSONObject().put("operation", "save_model_config").put("config", config.toJson()))
  }

  fun loadModelConfig(): RuntimeModelConfig? {
    val envelope = responseEnvelope(
      native.invoke(handle, JSONObject().put("operation", "load_model_config").toString()),
    )
    val data = envelope.opt("data")
    return if (data == null || data == JSONObject.NULL) null else (data as JSONObject).toModelConfig()
  }

  fun sendMessage(sessionId: String, content: String, apiKey: String?): RuntimeTurn {
    val request = JSONObject().put("session_id", sessionId).put("content", content)
    val data = responseData(native.sendMessage(handle, request.toString(), apiKey))
    return RuntimeTurn(data.getString("assistant_text"))
  }

  override fun close() {
    responseEnvelope(native.close(handle))
  }

  private fun invoke(request: JSONObject): JSONObject = responseData(native.invoke(handle, request.toString()))

  private fun invokeArray(request: JSONObject): JSONArray =
    responseEnvelope(native.invoke(handle, request.toString())).getJSONArray("data")
}

class RuntimeBridgeException(message: String) : IllegalStateException(message)

private fun RuntimeInitRequest.toJson(): JSONObject =
  JSONObject()
    .put("app_data_dir", appDataDir)
    .put("cache_dir", cacheDir)
    .put("database_path", databasePath)
    .put("skills_dir", skillsDir)
    .put("platform", platform)
    .put("capabilities", JSONArray(capabilities))

private fun RuntimeModelConfig.toJson(): JSONObject =
  JSONObject()
    .put("provider_id", providerId)
    .put("provider_name", providerName)
    .put("endpoint_type", endpointType)
    .put("base_url", baseUrl)
    .put("model_name", modelName)
    .put("secret_id", secretId)
    .put("headers", JSONObject(headers))

private fun responseEnvelope(response: String): JSONObject {
  val envelope = JSONObject(response)
  if (!envelope.optBoolean("ok")) {
    throw RuntimeBridgeException(envelope.optJSONObject("error")?.optString("message") ?: "Runtime call failed")
  }
  return envelope
}

private fun responseData(response: String): JSONObject = responseEnvelope(response).getJSONObject("data")

private fun JSONArray.strings(): List<String> = List(length()) { getString(it) }

private fun JSONArray.objects(): List<JSONObject> = List(length()) { getJSONObject(it) }

private fun JSONObject.toSession(): RuntimeSession =
  RuntimeSession(
    id = getString("id"),
    title = getString("title"),
    createdAt = getString("created_at"),
    updatedAt = getString("updated_at"),
  )

private fun JSONObject.toMessage(): RuntimeMessage =
  RuntimeMessage(
    id = getString("id"),
    sessionId = getString("session_id"),
    role = getString("role"),
    content = getString("content"),
    createdAt = getString("created_at"),
  )

private fun JSONObject.toModelConfig(): RuntimeModelConfig =
  RuntimeModelConfig(
    providerId = getString("provider_id"),
    providerName = getString("provider_name"),
    endpointType = getString("endpoint_type"),
    baseUrl = getString("base_url"),
    modelName = getString("model_name"),
    secretId = optString("secret_id").takeIf { it.isNotEmpty() },
    headers = getJSONObject("headers").keys().asSequence().associateWith { key ->
      getJSONObject("headers").getString(key)
    },
  )
