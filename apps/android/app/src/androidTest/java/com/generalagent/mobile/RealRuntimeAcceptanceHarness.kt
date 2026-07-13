package com.generalagent.mobile

import android.content.Context
import android.content.ContextWrapper
import android.os.Bundle
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.ui.Modifier
import androidx.test.core.app.ActivityScenario
import androidx.test.platform.app.InstrumentationRegistry
import com.generalagent.mobile.runtime.RuntimeActorContext
import com.generalagent.mobile.runtime.RuntimeBridge
import com.generalagent.mobile.runtime.RuntimeClient
import com.generalagent.mobile.runtime.RuntimeModelConfig
import com.generalagent.mobile.runtime.RuntimeSkillDraftFile
import com.generalagent.mobile.runtime.RuntimeSkillDraftRequest
import com.generalagent.mobile.runtime.RuntimeSkillPolicy
import com.generalagent.mobile.runtime.RuntimeSkillRollbackOutcome
import com.generalagent.mobile.secrets.InMemoryModelSecretStore
import com.generalagent.mobile.ui.AppRoot
import com.generalagent.mobile.ui.GeneralAgentTheme
import com.generalagent.mobile.ui.RuntimeSettingsGate
import com.generalagent.mobile.ui.RuntimeTurnGate
import com.generalagent.mobile.ui.ownerSkillInventory
import java.io.File
import java.security.MessageDigest
import java.util.UUID
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue

private const val LIFECYCLE_PACKAGE = "com.example.task17-mobile"
private const val RETAINED_PACKAGE = "com.example.task17-retained"
private const val ACTIVE_INSTRUCTION = "TASK17_UI_ACTIVE_SKILL_EVIDENCE"

internal fun runAuthoritativeNativeLifecycleTransitions() {
  val target = InstrumentationRegistry.getInstrumentation().targetContext
  val root = File(target.cacheDir, "native-lifecycle-${UUID.randomUUID()}")
  val context = IsolatedRuntimeContext(target, root)
  val client = runtimeBridge(context).load()
  try {
    val revisions = seedLifecycle(client)
    assertEquals(revisions.second, activeRevision(client, LIFECYCLE_PACKAGE))

    val rollback = client.rollbackManagedSkill(LIFECYCLE_PACKAGE, revisions.first)
      as RuntimeSkillRollbackOutcome.ApprovalRequired
    client.resolveSkillApproval(rollback.approval.approvalId, true)
    assertEquals(revisions.first, activeRevision(client, LIFECYCLE_PACKAGE))

    client.disableManagedSkill(LIFECYCLE_PACKAGE)
    assertEquals("disabled", managedStatus(client, LIFECYCLE_PACKAGE))
    assertFalse(client.listSkills().any { it.packageId == LIFECYCLE_PACKAGE && it.available })

    assertTrue(client.validateSkillDraft(revisions.third).ok)
    val reactivation = client.requestSkillActivation(revisions.third)
    client.resolveSkillApproval(reactivation.approvalId, true)
    assertEquals(revisions.third, activeRevision(client, LIFECYCLE_PACKAGE))

    val removal = client.requestSkillRemoval(LIFECYCLE_PACKAGE)
    client.resolveSkillApproval(removal.approvalId, true)
    val inventory = ownerSkillInventory(client.listSkills(), client.listManagedSkills())
    assertFalse(inventory.any { it.packageId == LIFECYCLE_PACKAGE })
  } finally {
    client.close()
    root.deleteRecursively()
  }
}

internal fun runRealRuntimeVisualHarness(arguments: Bundle) {
  val context = InstrumentationRegistry.getInstrumentation().targetContext
  val bridge = runtimeBridge(context)
  val client = bridge.load()
  val phase = arguments.getString("acceptance_phase") ?: "manual"
  try {
    if (arguments.getString("seed_runtime") != "false") {
      seedLifecycle(client)
      seedRetained(client)
    }
    val secrets = InMemoryModelSecretStore()
    arguments.getString("mock_base_url")?.let { baseUrl ->
      val secretId = "task17.mobile.mock"
      secrets.saveSecret(secretId, "task17-test-key")
      client.saveModelConfig(
        RuntimeModelConfig(
          providerId = "task17-mock",
          providerName = "Task17 mock",
          endpointType = "responses",
          baseUrl = baseUrl,
          modelName = "task17-model",
          secretId = secretId,
        ),
      )
    }
    writeAcceptanceState(context, client, "$phase-before")
    val turnGate = RuntimeTurnGate()
    val settingsGate = RuntimeSettingsGate()
    ActivityScenario.launch(MainActivity::class.java).use { scenario ->
      scenario.onActivity { activity ->
        activity.setContent {
          GeneralAgentTheme {
            AppRoot(
              runtimeClient = client,
              turnGate = turnGate,
              settingsGate = settingsGate,
              initialDiagnostics = client.diagnostics(),
              secretStore = secrets,
              modifier = Modifier.safeDrawingPadding(),
            )
          }
        }
      }
      Thread.sleep(arguments.getString("visual_wait_ms")?.toLongOrNull() ?: 240_000L)
    }
    writeAcceptanceState(context, client, "$phase-after")
    turnGate.close()
    settingsGate.close()
  } finally {
    client.close()
  }
}

private fun runtimeBridge(context: Context): RuntimeBridge {
  val grants = listOf(
    "inspect",
    "create_draft",
    "edit_draft",
    "validate",
    "activate",
    "disable",
    "rollback",
    "delete_managed",
  )
  val policy = RuntimeSkillPolicy(
    mode = "owner_only",
    agentAuthoring = true,
    allowedKinds = listOf("instruction_only", "host_tools_only"),
    rollbackApprovalRequired = true,
  )
  return RuntimeBridge(
    context = context,
    configuredSkillPolicy = policy,
    configuredActorContext = RuntimeActorContext(
      actorId = "android-requester",
      role = "owner",
      grants = grants,
    ),
    configuredApproverContext = RuntimeActorContext(
      actorId = "android-approver",
      role = "owner",
      grants = listOf("activate", "rollback", "delete_managed", "override_builtin"),
    ),
  )
}

private fun seedLifecycle(client: RuntimeClient): Triple<String, String, String> {
  val existing = client.listManagedSkills().find { it.packageId == LIFECYCLE_PACKAGE }
  if (existing != null) {
    val revisions = client.getSkillDetail(LIFECYCLE_PACKAGE).revisions
    val immutable = revisions.filter { !it.editable }.sortedBy { it.version }
    val draft = revisions.single { it.editable }
    return Triple(immutable.first().revisionId, immutable.last().revisionId, draft.revisionId)
  }
  val first = createValidatedDraft(client, LIFECYCLE_PACKAGE, "1.0.0", "Initial mobile instructions")
  activate(client, first)
  val second = createValidatedDraft(client, LIFECYCLE_PACKAGE, "2.0.0", ACTIVE_INSTRUCTION)
  activate(client, second)
  val draft = createValidatedDraft(client, LIFECYCLE_PACKAGE, "3.0.0", "Reactivated mobile instructions")
  return Triple(first, second, draft)
}

private fun seedRetained(client: RuntimeClient): String {
  client.listManagedSkills().find { it.packageId == RETAINED_PACKAGE }?.activeRevisionId?.let {
    return it
  }
  val revision = createValidatedDraft(client, RETAINED_PACKAGE, "1.0.0", "Retained managed instructions")
  activate(client, revision)
  return revision
}

private fun createValidatedDraft(
  client: RuntimeClient,
  packageId: String,
  version: String,
  instructions: String,
): String {
  val displayName = if (packageId == LIFECYCLE_PACKAGE) "Task17 mobile lifecycle" else "Task17 retained"
  val draft = client.createSkillDraft(
    RuntimeSkillDraftRequest(
      packageId = packageId,
      displayName = displayName,
      description = "Task17 Android acceptance package",
      kind = "instruction_only",
      requiredTools = emptyList(),
      initialFiles = listOf(
        RuntimeSkillDraftFile(
          "SKILL.md",
          "---\nname: $displayName\ndescription: Task17 Android acceptance package.\n---\n\n$instructions",
        ),
        RuntimeSkillDraftFile("general-agent.json", descriptor(packageId, displayName, version)),
      ),
    ),
  )
  val validation = client.validateSkillDraft(draft.revisionId)
  assertTrue(validation.errors.joinToString(), validation.ok)
  return draft.revisionId
}

private fun activate(client: RuntimeClient, revisionId: String) {
  val approval = client.requestSkillActivation(revisionId)
  client.resolveSkillApproval(approval.approvalId, true)
}

private fun descriptor(packageId: String, displayName: String, version: String): String =
  JSONObject()
    .put("schemaVersion", 1)
    .put("id", packageId)
    .put("version", version)
    .put("displayName", displayName)
    .put("kind", "instruction_only")
    .put("package", JSONObject().put("includeInstructions", true).put("includeRuntime", false))
    .put("compatibility", JSONObject().put("minimumRuntimeVersion", JSONObject.NULL).put("platforms", JSONArray()))
    .put(
      "requires",
      JSONObject()
        .put("packages", JSONArray())
        .put("capabilities", JSONArray())
        .put("runtimeTools", JSONArray())
        .put("connectors", JSONArray()),
    )
    .toString(2)

private fun activeRevision(client: RuntimeClient, packageId: String): String? =
  client.listManagedSkills().find { it.packageId == packageId }?.activeRevisionId

private fun managedStatus(client: RuntimeClient, packageId: String): String? =
  client.listManagedSkills().find { it.packageId == packageId }?.status

private fun writeAcceptanceState(context: Context, client: RuntimeClient, phase: String) {
  val managed = client.listManagedSkills()
  val root = context.getExternalFilesDir(null) ?: context.filesDir
  val init = runtimeBridge(context).initRequest()
  val lock = File(init.builtinSkillsDir)
    .walkTopDown()
    .firstOrNull { file -> file.name == "skill-bundle.lock" && file.isFile }
  val state = JSONObject()
    .put("phase", phase)
    .put("bundle_lock_sha256", lock?.let(::sha256) ?: JSONObject.NULL)
    .put(
      "managed",
      JSONArray(managed.map { skill ->
        JSONObject()
          .put("package_id", skill.packageId)
          .put("status", skill.status)
          .put("version", skill.version)
          .put("active_revision_id", skill.activeRevisionId)
      }),
    )
  File(root, "task17-$phase.json").writeText(state.toString(2))
}

private fun sha256(file: File): String = MessageDigest.getInstance("SHA-256")
  .digest(file.readBytes())
  .joinToString("") { byte -> "%02x".format(byte.toInt() and 0xff) }

private class IsolatedRuntimeContext(base: Context, root: File) : ContextWrapper(base) {
  private val files = File(root, "files").apply { mkdirs() }
  private val cache = File(root, "cache").apply { mkdirs() }

  override fun getFilesDir(): File = files

  override fun getCacheDir(): File = cache
}
