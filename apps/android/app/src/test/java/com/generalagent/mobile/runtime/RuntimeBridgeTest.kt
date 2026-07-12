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
  fun ownerOperationsUseStoredActorAndMapRuntimeDtos() {
    val native = OwnerNativeRuntime()
    val client = RuntimeClient(9L, native, skillGrants = setOf("inspect", "activate"))

    val managed = client.listManagedSkills().single()
    val detail = client.getSkillDetail("com.example.owner")
    val draft = client.createSkillDraft(
      RuntimeSkillDraftRequest(
        packageId = "com.example.new-skill",
        displayName = "New skill",
        description = "Owner authored instructions",
        kind = "instruction_only",
        requiredTools = listOf("host/search"),
      ),
    )
    client.updateSkillDraft(
      revisionId = draft.revisionId,
      files = listOf(RuntimeSkillDraftFile("instructions.md", "Updated instructions")),
    )
    val validation = client.validateSkillDraft(draft.revisionId)
    val approval = client.requestSkillActivation(draft.revisionId)
    val reload = client.resolveSkillApproval(approval.approvalId, approve = true)
    client.disableManagedSkill(managed.packageId)
    client.rollbackManagedSkill(managed.packageId, detail.revisions.last().revisionId)
    client.requestSkillRemoval(managed.packageId)

    assertEquals(setOf("inspect", "activate"), client.skillGrants)
    assertEquals("managed", managed.sourceLayer)
    assertEquals("host_tools_only", detail.revisions.first().kind)
    assertEquals("Draft instructions", detail.editableDraft?.instructions)
    assertTrue(validation.ok)
    assertEquals(listOf("host/search"), validation.requiredTools)
    assertEquals("approval-1", approval.approvalId)
    assertEquals(8L, reload.activeGeneration)
    assertTrue(native.requests.none { request ->
      request.has("actor") || request.has("actor_context") || request.has("principal")
    })
    assertEquals(
      listOf(
        "list_managed_skills",
        "get_skill_detail",
        "create_skill_draft",
        "update_skill_draft",
        "validate_skill_draft",
        "request_skill_activation",
        "resolve_skill_approval",
        "disable_managed_skill",
        "rollback_managed_skill",
        "request_skill_removal",
      ),
      native.requests.map { it.getString("operation") },
    )
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

private class OwnerNativeRuntime : NativeRuntimeApi {
  val requests = mutableListOf<JSONObject>()

  override fun initialize(requestJson: String): String = error("not used")

  override fun invoke(handle: Long, requestJson: String): String {
    val request = JSONObject(requestJson)
    requests += request
    val data = when (request.getString("operation")) {
      "list_managed_skills" -> """[{"package_id":"com.example.owner","display_name":"Owner skill","version":"1.0.0","source_layer":"managed","status":"active","reason":"","active_revision_id":"revision-active"}]"""
      "get_skill_detail" -> """{"package_id":"com.example.owner","display_name":"Owner skill","version":"1.0.0","source_layer":"managed","status":"active","reason":"","active_revision_id":"revision-active","revisions":[{"revision_id":"revision-draft","version":"1.1.0","status":"staging","editable":true,"created_by":"owner","created_at":"2026-07-13T00:00:00Z","kind":"host_tools_only","instructions":"Draft instructions","validation":{"ok":true,"errors":[],"warnings":[]},"requirements":{"runtime_tools":["host/search"],"capabilities":["network.http"],"connectors":[],"packages":[]},"permission_diff":{"capabilities":{"added":["network.http"]}}},{"revision_id":"revision-active","version":"1.0.0","status":"managed","editable":false,"created_by":"owner","created_at":"2026-07-12T00:00:00Z","kind":"host_tools_only","instructions":"Active instructions","validation":{"ok":true,"errors":[],"warnings":[]},"requirements":{"runtime_tools":["host/search"],"capabilities":[],"connectors":[],"packages":[]},"permission_diff":{}}],"editable_draft":{"revision_id":"revision-draft","version":"1.1.0","status":"staging","editable":true,"created_by":"owner","created_at":"2026-07-13T00:00:00Z","kind":"host_tools_only","instructions":"Draft instructions","validation":{"ok":true,"errors":[],"warnings":[]},"requirements":{"runtime_tools":["host/search"],"capabilities":["network.http"],"connectors":[],"packages":[]},"permission_diff":{}}}"""
      "create_skill_draft", "update_skill_draft" -> """{"package_id":"com.example.new-skill","revision_id":"revision-draft","version":"0.1.0","kind":"instruction_only","validation":{"status":"pending"},"status":"draft"}"""
      "validate_skill_draft" -> """{"ok":true,"errors":[],"warnings":[],"requiredTools":["host/search"],"requiredConnectors":[],"dependencies":[],"requiredCapabilities":["network.http"],"resolverStatus":"active","resolverErrors":[],"permissionDiff":{"capabilities":{"added":["network.http"]}},"revisionId":"revision-draft","contentHash":"hash","snapshotGeneration":7}"""
      "request_skill_activation", "request_skill_removal" -> """{"approval_id":"approval-1","package_id":"com.example.owner","permission_diff":{},"requested_by":"owner","revision_id":"revision-draft","status":"pending"}"""
      "resolve_skill_approval" -> """{"previous_generation":7,"active_generation":8,"active_packages":1,"inactive_packages":0,"status":"approved"}"""
      "disable_managed_skill" -> """{"previous_generation":8,"active_generation":9,"active_packages":0,"inactive_packages":1}"""
      "rollback_managed_skill" -> """{"package_id":"com.example.owner","active_revision_id":"revision-active","replaced_revision_id":"revision-new","generation":10}"""
      else -> error("unexpected operation")
    }
    return """{"ok":true,"data":$data}"""
  }

  override fun sendMessage(handle: Long, requestJson: String, apiKey: String?): String =
    error("not used")

  override fun close(handle: Long): String = """{"ok":true,"data":null}"""
}
