package com.generalagent.mobile

import androidx.activity.compose.setContent
import androidx.test.core.app.ActivityScenario
import androidx.test.platform.app.InstrumentationRegistry
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.ui.Modifier
import com.generalagent.mobile.runtime.NativeRuntimeApi
import com.generalagent.mobile.runtime.RuntimeClient
import com.generalagent.mobile.runtime.RuntimeDiagnostics
import com.generalagent.mobile.secrets.InMemoryModelSecretStore
import com.generalagent.mobile.ui.AppTab
import com.generalagent.mobile.ui.AppRoot
import com.generalagent.mobile.ui.GeneralAgentTheme
import com.generalagent.mobile.ui.RuntimeSettingsGate
import com.generalagent.mobile.ui.RuntimeTurnGate
import com.generalagent.mobile.ui.SkillAction
import com.generalagent.mobile.ui.SkillScreenMode
import com.generalagent.mobile.ui.skillAccessState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.json.JSONObject

class MainActivityInstrumentedTest {
  @Test
  fun disabledAndOwnerPolicyStatesAreStableOnDevice() {
    val disabled = skillAccessState("disabled", emptySet())
    val owner = skillAccessState(
      "owner_only",
      setOf("inspect", "create_draft", "activate"),
    )

    assertFalse(AppTab.Skills in disabled.visibleTabs)
    assertEquals(SkillScreenMode.OwnerManage, owner.mode)
    assertTrue(SkillAction.Create in owner.actions)
    assertTrue(SkillAction.Activate in owner.actions)
    assertFalse(SkillAction.Delete in owner.actions)
  }

  @Test
  fun defaultApplicationLaunchesOnTheDevice() {
    val packageName = InstrumentationRegistry.getInstrumentation().targetContext.packageName
    assertEquals("com.generalagent.mobile", packageName)

    ActivityScenario.launch(MainActivity::class.java).use { scenario ->
      scenario.onActivity { activity ->
        assertTrue(activity.window.decorView.isShown)
      }
    }
  }

  @Test
  fun visualSkillManagementHarness() {
    val arguments = InstrumentationRegistry.getArguments()
    if (arguments.getString("visual_harness") != "true") return
    val mode = arguments.getString("skill_mode") ?: "owner_only"
    val grants = if (mode == "owner_only") {
      setOf(
        "inspect",
        "create_draft",
        "edit_draft",
        "validate",
        "activate",
        "disable",
        "rollback",
        "delete_managed",
      )
    } else {
      emptySet()
    }
    val diagnostics = visualDiagnostics(mode)
    val client = RuntimeClient(77L, VisualNativeRuntime(mode), grants)
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
              initialDiagnostics = diagnostics,
              secretStore = InMemoryModelSecretStore(),
              modifier = Modifier.safeDrawingPadding(),
            )
          }
        }
      }
      Thread.sleep(arguments.getString("visual_wait_ms")?.toLongOrNull() ?: 240_000L)
    }
    turnGate.close()
    settingsGate.close()
    client.close()
  }
}

private fun visualDiagnostics(mode: String) = RuntimeDiagnostics(
  platform = "android",
  capabilities = listOf(
    "network.http",
    "filesystem.app_data",
    "secure_storage",
    "model.http_provider",
  ),
  databaseReady = true,
  skillsReady = true,
  modelConfigured = false,
  skillManagementMode = mode,
  activeSnapshotGeneration = 7,
  quarantinedCount = 0,
  lastReloadStatus = "generation:7",
)

private class VisualNativeRuntime(private val mode: String) : NativeRuntimeApi {
  private var validationCount = 0

  override fun initialize(requestJson: String): String = error("not used")

  override fun invoke(handle: Long, requestJson: String): String {
    val request = JSONObject(requestJson)
    val data = when (request.getString("operation")) {
      "diagnostics" -> diagnosticsJson()
      "list_skills" -> skillInventoryJson()
      "list_managed_skills" -> if (mode == "owner_only") managedInventoryJson() else "[]"
      "get_skill_detail" -> detailJson(request.getString("package_id"))
      "create_skill_draft", "update_skill_draft" -> draftSummaryJson()
      "validate_skill_draft" -> validationJson(++validationCount > 1)
      "request_skill_activation", "request_skill_removal" -> approvalJson(request)
      "resolve_skill_approval" -> reloadJson()
      "disable_managed_skill" -> reloadJson()
      "rollback_managed_skill" -> rollbackApprovalJson(request)
      "list_sessions" -> "[]"
      "create_session" -> sessionJson(request.optString("title", "New chat"))
      "get_messages" -> "[]"
      "load_model_config" -> "null"
      "save_model_config", "delete_session" -> "null"
      else -> error("unexpected visual operation: ${request.getString("operation")}")
    }
    return """{"ok":true,"data":$data}"""
  }

  override fun sendMessage(handle: Long, requestJson: String, apiKey: String?): String =
    """{"ok":true,"data":{"assistant_text":"Visual fixture reply"}}"""

  override fun close(handle: Long): String = """{"ok":true,"data":null}"""

  private fun diagnosticsJson(): String =
    """{"platform":"android","capabilities":["network.http","filesystem.app_data","secure_storage","model.http_provider"],"database_ready":true,"skills_ready":true,"model_configured":false,"skill_management_mode":"$mode","active_snapshot_generation":7,"quarantined_count":0,"last_reload_status":"generation:7"}"""

  private fun skillInventoryJson(): String = """[
    {"package_id":"com.generalagent.research","display_name":"Research assistant","version":"1.4.0","source_layer":"managed","status":"active","available":true,"reason":"","active_revision_id":"8a41912f-a83c-4fd8-a3b4-123456789abc","manageable":true},
    {"package_id":"com.generalagent.calendar","display_name":"Calendar briefing","version":"1.0.0","source_layer":"builtin","status":"capability_missing","available":false,"reason":"Missing required capability: calendar.read","active_revision_id":null,"manageable":false},
    {"package_id":"com.generalagent.notes","display_name":"Meeting notes","version":"2.1.0","source_layer":"builtin","status":"active","available":true,"reason":"","active_revision_id":null,"manageable":false}
  ]"""

  private fun managedInventoryJson(): String = """[
    {"package_id":"com.generalagent.research","display_name":"Research assistant","version":"1.4.0","source_layer":"managed","status":"active","reason":"","active_revision_id":"8a41912f-a83c-4fd8-a3b4-123456789abc"}
  ]"""

  private fun detailJson(packageId: String): String {
    if (packageId != "com.generalagent.research") {
      return """{"package_id":"$packageId","display_name":"Built-in package","version":"1.0.0","source_layer":"builtin","status":"active","reason":"","active_revision_id":null,"revisions":[],"editable_draft":null}"""
    }
    val draft = revisionJson(
      id = "b71c6c72-8893-49f4-b593-111111111111",
      version = "1.5.0",
      status = "staging",
      editable = true,
      instructions = "Summarize trusted sources and cite every factual claim.",
      valid = false,
    )
    val active = revisionJson(
      id = "8a41912f-a83c-4fd8-a3b4-123456789abc",
      version = "1.4.0",
      status = "managed",
      editable = false,
      instructions = "Research a topic using approved host tools.",
      valid = true,
    )
    val old = revisionJson(
      id = "32af842d-e04d-49a1-a1b5-222222222222",
      version = "1.3.0",
      status = "managed",
      editable = false,
      instructions = "Research and summarize.",
      valid = true,
    )
    return """{"package_id":"com.generalagent.research","display_name":"Research assistant","version":"1.4.0","source_layer":"managed","status":"active","reason":"","active_revision_id":"8a41912f-a83c-4fd8-a3b4-123456789abc","revisions":[$draft,$active,$old],"editable_draft":$draft}"""
  }

  private fun revisionJson(
    id: String,
    version: String,
    status: String,
    editable: Boolean,
    instructions: String,
    valid: Boolean,
  ): String = """{"revision_id":"$id","version":"$version","status":"$status","editable":$editable,"created_by":"android-owner","created_at":"2026-07-13T09:30:00Z","kind":"host_tools_only","instructions":"$instructions","validation":{"ok":$valid,"errors":${if (valid) "[]" else "[\"Tool host/search is unavailable in this snapshot\"]"},"warnings":[]},"requirements":{"runtime_tools":["host/search","host/read"],"capabilities":["network.http","filesystem.app_data"],"connectors":[],"packages":[]},"permission_diff":{"capabilities":{"added":["network.http"]}}}"""

  private fun draftSummaryJson(): String =
    """{"package_id":"com.generalagent.research","revision_id":"b71c6c72-8893-49f4-b593-111111111111","version":"1.5.0","kind":"host_tools_only","validation":{"status":"pending"},"status":"draft"}"""

  private fun validationJson(valid: Boolean): String =
    """{"ok":$valid,"errors":${if (valid) "[]" else "[\"Tool host/search is unavailable in this snapshot\"]"},"warnings":[],"requiredTools":["host/search","host/read"],"requiredConnectors":[],"dependencies":[],"requiredCapabilities":["network.http","filesystem.app_data"],"resolverStatus":"${if (valid) "active" else "capability_missing"}","resolverErrors":[],"permissionDiff":{"capabilities":{"added":["network.http"]}},"revisionId":"b71c6c72-8893-49f4-b593-111111111111","contentHash":"visual-hash","snapshotGeneration":7}"""

  private fun approvalJson(request: JSONObject): String {
    val revision = request.optString("revision_id", "8a41912f-a83c-4fd8-a3b4-123456789abc")
    return """{"approval_id":"a9cb37b2-40d0-46d5-8fb2-333333333333","package_id":"com.generalagent.research","permission_diff":{"capabilities":{"added":["network.http"]}},"requested_by":"android-owner","revision_id":"$revision","status":"pending"}"""
  }

  private fun rollbackApprovalJson(request: JSONObject): String =
    """{"approval_id":"d83a3886-dffd-48f8-bd52-444444444444","package_id":"${request.getString("package_id")}","revision_id":"${request.getString("revision_id")}","status":"pending"}"""

  private fun reloadJson(): String =
    """{"previous_generation":7,"active_generation":8,"active_packages":2,"inactive_packages":1,"status":"approved"}"""

  private fun sessionJson(title: String): String =
    """{"id":"visual-session","title":"$title","created_at":"2026-07-13T09:00:00Z","updated_at":"2026-07-13T09:00:00Z"}"""
}
