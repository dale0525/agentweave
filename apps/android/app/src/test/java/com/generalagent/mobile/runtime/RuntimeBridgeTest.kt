package com.generalagent.mobile.runtime

import android.content.Context
import java.io.ByteArrayInputStream
import java.io.InputStream
import java.nio.charset.StandardCharsets
import java.security.MessageDigest
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
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

    val client = RuntimeBridge(
      context,
      native,
      BridgeSkillAssets(),
      publicationFileSystem = JvmSkillPublicationFileSystem(),
    ).load()
    val request = JSONObject(native.initializeRequest)

    assertEquals(41L, client.handle)
    assertEquals(context.filesDir.absolutePath, request.getString("app_data_dir"))
    assertEquals(context.cacheDir.absolutePath, request.getString("cache_dir"))
    assertTrue(request.getString("database_path").startsWith(context.filesDir.absolutePath))
    assertTrue(request.getString("builtin_skills_dir").startsWith(context.filesDir.absolutePath))
    assertTrue(request.getString("managed_skills_dir").startsWith(context.filesDir.absolutePath))
    assertTrue(request.getString("staging_skills_dir").startsWith(context.cacheDir.absolutePath))
    assertTrue(request.getString("quarantine_skills_dir").startsWith(context.filesDir.absolutePath))
    assertEquals("disabled", request.getJSONObject("skill_policy").getString("mode"))
    assertEquals("anonymous", request.getJSONObject("actor_context").getString("actor_id"))
    assertEquals("android", request.getString("platform"))
    assertEquals(4, request.getJSONArray("capabilities").length())
    assertFalse(native.initializeRequest.contains("api_key", ignoreCase = true))
  }

  @Test
  fun configuredPolicyAndActorAreStoredOnlyInInitialization() {
    val context = RuntimeEnvironment.getApplication() as Context
    val native = RecordingNativeRuntime()
    val policy = RuntimeSkillPolicy(
      mode = "owner_only",
      agentAuthoring = true,
      allowedKinds = listOf("instruction_only"),
      activationApprovalRequired = false,
    )
    val actor = RuntimeActorContext(
      actorId = "android-owner",
      role = "owner",
      tenantId = "tenant-1",
      deviceId = "device-1",
      grants = listOf("inspect", "create_draft"),
    )

    RuntimeBridge(
      context,
      native,
      BridgeSkillAssets(),
      policy,
      actor,
      JvmSkillPublicationFileSystem(),
    ).load()
    val request = JSONObject(native.initializeRequest)

    assertEquals("owner_only", request.getJSONObject("skill_policy").getString("mode"))
    assertEquals("android-owner", request.getJSONObject("actor_context").getString("actor_id"))
    assertFalse(native.initializeRequest.contains("actor_override"))
  }

  @Test
  fun loadModelConfigPreservesNullSecretReference() {
    val native = object : NativeRuntimeApi {
      override fun initialize(requestJson: String): String = error("not used")

      override fun invoke(handle: Long, requestJson: String): String =
        """{"ok":true,"data":{"provider_id":"local","provider_name":"Local","endpoint_type":"responses","base_url":"http://localhost:11434/v1","model_name":"qwen","secret_id":null,"headers":{}}}"""

      override fun sendMessage(handle: Long, requestJson: String, apiKey: String?): String =
        error("not used")

      override fun close(handle: Long): String = """{"ok":true,"data":null}"""
    }

    val config = RuntimeClient(9L, native).loadModelConfig()

    assertNull(config?.secretId)
  }

  @Test
  fun listSkillsPreservesRuntimeAvailabilityReasons() {
    val native = object : NativeRuntimeApi {
      override fun initialize(requestJson: String): String = error("not used")

      override fun invoke(handle: Long, requestJson: String): String =
        """{"ok":true,"data":[{"package_id":"com.example.web-browser","display_name":"Web browser","version":"1.2.3","source_layer":"builtin","status":"capability_missing","available":false,"reason":"Missing required capability: browser.headless","active_revision_id":null,"manageable":false}]}"""

      override fun sendMessage(handle: Long, requestJson: String, apiKey: String?): String =
        error("not used")

      override fun close(handle: Long): String = """{"ok":true,"data":null}"""
    }

    val skills = RuntimeClient(9L, native).listSkills()

    assertEquals(1, skills.size)
    assertEquals("com.example.web-browser", skills.single().packageId)
    assertEquals("builtin", skills.single().sourceLayer)
    assertEquals("capability_missing", skills.single().status)
    assertFalse(skills.single().available)
    assertEquals("Missing required capability: browser.headless", skills.single().reason)
  }

  @Test
  fun saveModelConfigAcceptsNullUnitPayload() {
    val native = object : NativeRuntimeApi {
      override fun initialize(requestJson: String): String = error("not used")

      override fun invoke(handle: Long, requestJson: String): String =
        """{"ok":true,"data":null}"""

      override fun sendMessage(handle: Long, requestJson: String, apiKey: String?): String =
        error("not used")

      override fun close(handle: Long): String = """{"ok":true,"data":null}"""
    }

    RuntimeClient(9L, native).saveModelConfig(
      RuntimeModelConfig(
        providerId = "local",
        providerName = "Local",
        endpointType = "responses",
        baseUrl = "http://localhost:11434/v1",
        modelName = "qwen",
        secretId = null,
      ),
    )
  }

  @Test
  fun malformedSuccessfulInitializationClosesAllocatedHandle() {
    val context = RuntimeEnvironment.getApplication() as Context
    val native = RecordingNativeRuntime(
      initializeResponse = """{"ok":true,"data":{"handle":41,"unexpected":true}}""",
    )

    assertThrows(RuntimeBridgeException::class.java) {
      RuntimeBridge(
        context,
        native,
        BridgeSkillAssets(),
        publicationFileSystem = JvmSkillPublicationFileSystem(),
      ).load()
    }

    assertEquals(listOf(41L), native.closedHandles)
  }

  @Test
  fun runtimeClientCloseIsIdempotent() {
    val native = RecordingNativeRuntime()
    val client = RuntimeClient(41L, native)

    client.close()
    client.close()

    assertEquals(listOf(41L), native.closedHandles)
  }
}

private class BridgeSkillAssets : SkillAssetSource {
  private val files = mapOf(
    "current" to "current".toByteArray(StandardCharsets.UTF_8),
    "generations/current/skill-bundle.json" to "manifest".toByteArray(StandardCharsets.UTF_8),
    "generations/current/skill-bundle.lock" to "lock".toByteArray(StandardCharsets.UTF_8),
  )

  override fun bundleHash(): String {
    val digest = MessageDigest.getInstance("SHA-256")
    for ((path, bytes) in files.toSortedMap()) {
      digest.update(path.toByteArray(StandardCharsets.UTF_8))
      digest.update(0)
      digest.update(bytes.size.toString().toByteArray(StandardCharsets.US_ASCII))
      digest.update(0)
      digest.update(bytes)
    }
    return digest.digest().joinToString("") { byte -> "%02x".format(byte) }
  }

  override fun entries(): List<SkillAssetEntry> =
    files.keys.map { SkillAssetEntry(it, SkillAssetType.FILE) }

  override fun open(relativePath: String): InputStream =
    ByteArrayInputStream(checkNotNull(files[relativePath]))
}

private class RecordingNativeRuntime(
  private val initializeResponse: String = """{"ok":true,"data":{"handle":41}}""",
) : NativeRuntimeApi {
  var initializeRequest: String = ""
  val closedHandles = mutableListOf<Long>()

  override fun initialize(requestJson: String): String {
    initializeRequest = requestJson
    return initializeResponse
  }

  override fun invoke(handle: Long, requestJson: String): String =
    """{"ok":true,"data":null}"""

  override fun sendMessage(handle: Long, requestJson: String, apiKey: String?): String =
    """{"ok":true,"data":{"assistant_text":"ok"}}"""

  override fun close(handle: Long): String {
    closedHandles += handle
    return """{"ok":true,"data":null}"""
  }
}
