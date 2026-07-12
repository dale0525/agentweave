package com.generalagent.mobile.ui

import com.generalagent.mobile.runtime.RuntimeDiagnostics
import com.generalagent.mobile.runtime.RuntimeSkill
import com.generalagent.mobile.runtime.RuntimeSkillDetail
import com.generalagent.mobile.runtime.RuntimeSkillRequirements
import com.generalagent.mobile.runtime.RuntimeSkillRevision
import com.generalagent.mobile.runtime.RuntimeSkillPackageSummary
import com.generalagent.mobile.runtime.RuntimeSkillValidationSummary
import com.generalagent.mobile.runtime.RuntimeSkillValidation
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class SkillManagementStateTest {
  @Test
  fun disabledPolicyRemovesSkillsNavigation() {
    assertFalse(visibleTabs(skillManagementMode = "disabled").contains(AppTab.Skills))
  }

  @Test
  fun disabledPolicyRejectsAStaleSkillsSelection() {
    assertEquals(
      AppTab.Chat,
      admittedPolicyTab(
        current = AppTab.Skills,
        requested = AppTab.Skills,
        visibleTabs = visibleTabs("disabled"),
      ),
    )
  }

  @Test
  fun diagnosticsPolicyShowsReadOnlySkillsNavigation() {
    val state = skillScreenMode("diagnostics_only", grants = emptySet())

    assertEquals(SkillScreenMode.DiagnosticsOnly, state)
    assertTrue(skillActions(state, grants = allSkillGrants()).isEmpty())
  }

  @Test
  fun ownerPolicyRequiresInspectGrant() {
    assertEquals(
      SkillScreenMode.Hidden,
      skillScreenMode("owner_only", grants = setOf("activate")),
    )
  }

  @Test
  fun ownerPolicyRequiresManagementGrantsForActions() {
    val state = skillScreenMode("owner_only", grants = setOf("inspect", "activate"))

    assertEquals(SkillScreenMode.OwnerManage, state)
    assertTrue(skillActions(state, grants = setOf("activate")).contains(SkillAction.Activate))
    assertFalse(skillActions(state, grants = setOf("activate")).contains(SkillAction.Delete))
  }

  @Test
  fun ownerActionsMapEveryRuntimeGrantWithoutImpliedPermissions() {
    assertEquals(
      setOf(
        SkillAction.Create,
        SkillAction.Edit,
        SkillAction.Validate,
        SkillAction.Activate,
        SkillAction.Disable,
        SkillAction.Rollback,
        SkillAction.Delete,
      ),
      skillActions(SkillScreenMode.OwnerManage, allSkillGrants()),
    )
    assertEquals(
      setOf(SkillAction.Edit),
      skillActions(SkillScreenMode.OwnerManage, setOf("edit_draft")),
    )
  }

  @Test
  fun failedMutationRetainsInventoryAndActiveRevision() {
    val inventory = listOf(runtimeSkill(activeRevisionId = "revision-active"))
    val detail = skillDetail(activeRevisionId = "revision-active")
    val state = SkillManagementUiState(
      inventory = inventory,
      diagnostics = diagnostics(generation = 7),
      detail = detail,
      busyOperation = "activate",
    )

    val failed = skillOperationFailed(state, "Activation failed")

    assertEquals(inventory, failed.inventory)
    assertEquals("revision-active", failed.detail?.activeRevisionId)
    assertEquals(7L, failed.diagnostics.activeSnapshotGeneration)
    assertEquals(null, failed.busyOperation)
    assertEquals("Activation failed", failed.inlineError)
  }

  @Test
  fun draftSaveWritesInstructionsAndDescriptorWithoutActor() {
    val detail = skillDetail(activeRevisionId = "revision-active")
    val revision = detail.editableDraft!!

    val files = draftUpdateFiles(
      detail = detail,
      revision = revision,
      instructions = "Updated owner instructions",
      requiredTools = listOf("host/search", "host/read"),
    )

    assertEquals(listOf("SKILL.md", "general-agent.json"), files.map { it.path })
    assertEquals("Updated owner instructions", files.first().content)
    assertTrue(files.last().content.contains("\"runtimeTools\""))
    assertFalse(files.last().content.contains("actor", ignoreCase = true))
  }

  @Test
  fun approvalCapabilitiesPreferAddedPermissionDiff() {
    assertEquals(
      listOf("secure_storage", "network.http"),
      approvalCapabilities(
        permissionDiffJson = """{"capabilities":{"added":["secure_storage","network.http"]}}""",
        fallback = listOf("filesystem.app_data"),
      ),
    )
    assertEquals(
      listOf("filesystem.app_data"),
      approvalCapabilities(permissionDiffJson = "{}", fallback = listOf("filesystem.app_data")),
    )
  }

  @Test
  fun ownerInventoryIncludesDraftsOutsideTheEffectiveSnapshot() {
    val effective = listOf(runtimeSkill(activeRevisionId = "revision-active"))
    val staging = RuntimeSkillPackageSummary(
      packageId = "com.example.draft",
      displayName = "Draft skill",
      version = "0.1.0",
      sourceLayer = "managed",
      status = "draft",
      reason = "",
      activeRevisionId = null,
    )

    val inventory = ownerSkillInventory(effective, listOf(staging))

    assertEquals(listOf("com.example.draft", "com.example.owner"), inventory.map { it.packageId })
    assertFalse(inventory.first().available)
    assertTrue(inventory.first().manageable)
  }

  @Test
  fun activationRevisionUsesTheLatestValidationResult() {
    val revision = skillDetail(activeRevisionId = "revision-active").editableDraft!!
    val validation = RuntimeSkillValidation(
      ok = true,
      errors = emptyList(),
      warnings = listOf("Review network scope"),
      requiredTools = listOf("host/search", "host/read"),
      requiredConnectors = emptyList(),
      dependencies = emptyList(),
      requiredCapabilities = listOf("network.http", "filesystem.app_data"),
      resolverStatus = "active",
      resolverErrors = emptyList(),
      permissionDiffJson = """{"capabilities":{"added":["network.http"]}}""",
      revisionId = revision.revisionId,
      contentHash = "hash",
      snapshotGeneration = 7,
    )

    val merged = revisionAfterValidation(revision, validation)

    assertTrue(merged.validation.ok)
    assertEquals(emptyList<String>(), merged.validation.errors)
    assertEquals(listOf("host/search", "host/read"), merged.requirements.runtimeTools)
    assertEquals(listOf("network.http", "filesystem.app_data"), merged.requirements.capabilities)
    assertEquals(validation.permissionDiffJson, merged.permissionDiffJson)
  }

  private fun allSkillGrants(): Set<String> =
    setOf(
      "create_draft",
      "edit_draft",
      "validate",
      "activate",
      "disable",
      "rollback",
      "delete_managed",
    )

  private fun runtimeSkill(activeRevisionId: String) = RuntimeSkill(
    packageId = "com.example.owner",
    displayName = "Owner skill",
    version = "1.0.0",
    sourceLayer = "managed",
    status = "active",
    available = true,
    reason = "",
    activeRevisionId = activeRevisionId,
    manageable = true,
  )

  private fun skillDetail(activeRevisionId: String): RuntimeSkillDetail {
    val draft = RuntimeSkillRevision(
      revisionId = "revision-draft",
      version = "1.1.0",
      status = "staging",
      editable = true,
      createdBy = "owner",
      createdAt = "2026-07-13T00:00:00Z",
      kind = "instruction_only",
      instructions = "Draft instructions",
      validation = RuntimeSkillValidationSummary(false, listOf("Validation required"), emptyList()),
      requirements = RuntimeSkillRequirements(
        runtimeTools = listOf("host/search"),
        capabilities = listOf("network.http"),
        connectors = emptyList(),
        packages = emptyList(),
      ),
      permissionDiffJson = "{}",
    )
    return RuntimeSkillDetail(
      packageId = "com.example.owner",
      displayName = "Owner skill",
      version = "1.0.0",
      sourceLayer = "managed",
      status = "active",
      reason = "",
      activeRevisionId = activeRevisionId,
      revisions = listOf(draft),
      editableDraft = draft,
    )
  }

  private fun diagnostics(generation: Long) = RuntimeDiagnostics(
    platform = "android",
    capabilities = emptyList(),
    databaseReady = true,
    skillsReady = true,
    modelConfigured = false,
    skillManagementMode = "owner_only",
    activeSnapshotGeneration = generation,
    quarantinedCount = 0,
    lastReloadStatus = "generation:$generation",
  )
}
