package com.generalagent.mobile.ui

import com.generalagent.mobile.runtime.RuntimeDiagnostics
import com.generalagent.mobile.runtime.RuntimeActorContext
import com.generalagent.mobile.runtime.RuntimeSkill
import com.generalagent.mobile.runtime.RuntimeSkillPolicy
import com.generalagent.mobile.runtime.RuntimeSkillDetail
import com.generalagent.mobile.runtime.RuntimeSkillRequirements
import com.generalagent.mobile.runtime.RuntimeSkillRevision
import com.generalagent.mobile.runtime.RuntimeSkillPackageSummary
import com.generalagent.mobile.runtime.RuntimeSkillValidationSummary
import com.generalagent.mobile.runtime.RuntimeSkillValidation
import com.generalagent.mobile.runtime.RuntimeSkillApproval
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
  fun ownerPolicyRequiresOwnerRoleAndAuthoringPolicy() {
    val policy = RuntimeSkillPolicy(
      mode = "owner_only",
      agentAuthoring = false,
      allowedKinds = listOf("instruction_only"),
    )

    assertEquals(
      SkillScreenMode.Hidden,
      skillAccessState(policy, RuntimeActorContext(role = "user", grants = listOf("inspect"))).mode,
    )
    val owner = skillAccessState(
      policy,
      RuntimeActorContext(actorId = "owner", role = "owner", grants = listOf("inspect", "create_draft")),
    )
    assertEquals(SkillScreenMode.OwnerManage, owner.mode)
    assertFalse(SkillAction.Create in owner.actions)
  }

  @Test
  fun createActionRequiresAnAllowedAuthoringKind() {
    val actor = RuntimeActorContext(
      actorId = "owner",
      role = "owner",
      grants = listOf("inspect", "create_draft"),
    )
    assertFalse(
      SkillAction.Create in skillAccessState(
        RuntimeSkillPolicy(mode = "owner_only", agentAuthoring = true, allowedKinds = emptyList()),
        actor,
      ).actions,
    )
    assertTrue(
      SkillAction.Create in skillAccessState(
        RuntimeSkillPolicy(
          mode = "owner_only",
          agentAuthoring = true,
          allowedKinds = listOf("instruction_only"),
        ),
        actor,
      ).actions,
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
  fun publishedSynchronizationWarningIsRetryableWithoutClaimingApprovalFailure() {
    val message = publicationSynchronizationWarning("requester synchronization failed")

    assertTrue(message.startsWith("Published, refresh required"))
    assertFalse(message.contains("Approval failed"))
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
  fun permissionChangesUseTheAuthoritativeTopLevelSchemaWithoutFallback() {
    assertEquals(
      listOf(
        "+ capability: network.http",
        "- capability: filesystem.app_data",
        "+ tool: host/search",
        "- tool: host/write",
        "+ connector: com.example.calendar",
        "- connector: com.example.legacy",
      ),
      permissionChanges(
        """{"addedCapabilities":["network.http"],"removedCapabilities":["filesystem.app_data"],"addedTools":["host/search"],"removedTools":["host/write"],"addedConnectors":["com.example.calendar"],"removedConnectors":["com.example.legacy"]}""",
      ),
    )
    assertEquals(emptyList<String>(), permissionChanges("{}"))
    assertEquals(emptyList<String>(), permissionChanges("not-json"))
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
    assertFalse(inventory.first().manageable)
  }

  @Test
  fun ownerInventoryPreservesRuntimeManageability() {
    val immutable = runtimeSkill(activeRevisionId = "revision-active").copy(manageable = false)
    val summary = RuntimeSkillPackageSummary(
      packageId = immutable.packageId,
      displayName = immutable.displayName,
      version = immutable.version,
      sourceLayer = "managed",
      status = "active",
      reason = "",
      activeRevisionId = immutable.activeRevisionId,
    )

    assertFalse(ownerSkillInventory(listOf(immutable), listOf(summary)).single().manageable)
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

    val merged = revisionAfterValidation(
      revision,
      validation,
      savedInstructions = "B instructions",
      savedTools = listOf("host/search", "host/read"),
    )

    assertTrue(merged.validation.ok)
    assertEquals(emptyList<String>(), merged.validation.errors)
    assertEquals(listOf("host/search", "host/read"), merged.requirements.runtimeTools)
    assertEquals(listOf("network.http", "filesystem.app_data"), merged.requirements.capabilities)
    assertEquals(validation.permissionDiffJson, merged.permissionDiffJson)
    assertEquals("B instructions", merged.instructions)
    assertEquals("hash", merged.contentHash)
    val approvalState = SkillApprovalUiState(
      operation = SkillApprovalOperation.Activation,
      approval = RuntimeSkillApproval(
        approvalId = "approval-1",
        packageId = "com.example.owner",
        permissionDiffJson = validation.permissionDiffJson,
        requestedBy = "requester",
        revisionId = merged.revisionId,
        status = "pending",
      ),
      detail = skillDetail(activeRevisionId = "revision-active"),
      revision = merged,
    )
    assertEquals("B instructions", approvalState.revision.instructions)
    assertEquals(validation.contentHash, approvalState.revision.contentHash)
  }

  @Test
  fun targetActionsRespectManageabilityKindAndLifecycle() {
    val access = SkillAccessState(
      mode = SkillScreenMode.OwnerManage,
      visibleTabs = AppTab.entries,
      actions = setOf(
        SkillAction.Edit,
        SkillAction.Validate,
        SkillAction.Activate,
        SkillAction.Disable,
        SkillAction.Rollback,
        SkillAction.Delete,
      ),
      allowedKinds = setOf("instruction_only"),
    )
    val unmanaged = skillDetail(activeRevisionId = "revision-active").copy(sourceLayer = "builtin")
    assertTrue(skillTargetActions(access, unmanaged, manageable = false).isEmpty())

    val removed = skillDetail(activeRevisionId = "revision-active").copy(status = "removed")
    assertTrue(skillTargetActions(access, removed, manageable = true).isEmpty())

    val managed = skillDetailWithHistory()
    val actions = skillTargetActions(access, managed, manageable = true)
    assertTrue(SkillAction.Edit in actions)
    assertTrue(SkillAction.Validate in actions)
    assertTrue(SkillAction.Disable in actions)
    assertTrue(SkillAction.Delete in actions)
  }

  @Test
  fun targetActionsRejectProtectedDisallowedAndDraftOnlyLifecycle() {
    val detail = skillDetailWithHistory()
    val access = skillAccessState(
      RuntimeSkillPolicy(
        mode = "owner_only",
        agentAuthoring = true,
        allowedKinds = listOf("instruction_only"),
        protectedPackages = listOf(detail.packageId),
        allowedOverrides = listOf(detail.packageId),
      ),
      RuntimeActorContext(role = "owner", grants = listOf("inspect") + allSkillGrants()),
    )
    assertTrue(detail.packageId in access.allowedOverrides)
    assertTrue(skillTargetActions(access, detail, manageable = true).isEmpty())

    val disallowed = detail.copy(
      packageId = "com.example.host-tools",
      revisions = detail.revisions.map { it.copy(kind = "host_tools_only") },
      editableDraft = detail.editableDraft?.copy(kind = "host_tools_only"),
    )
    assertTrue(skillTargetActions(access.copy(protectedPackages = emptySet()), disallowed, true).isEmpty())

    val draftOnly = detail.copy(
      packageId = "com.example.draft-only",
      status = "draft",
      activeRevisionId = null,
      revisions = listOf(checkNotNull(detail.editableDraft)),
    )
    val draftActions = skillTargetActions(
      access.copy(protectedPackages = emptySet()),
      draftOnly,
      manageable = true,
    )
    assertFalse(SkillAction.Delete in draftActions)
    assertFalse(SkillAction.Disable in draftActions)
    assertFalse(SkillAction.Rollback in draftActions)
  }

  @Test
  fun hostToolsOnlyPolicyIsTheCreateDefaultAndRejectsOutOfPolicyKinds() {
    val allowed = linkedSetOf("host_tools_only")

    assertEquals("host_tools_only", initialDraftKind(allowed))
    assertTrue(admitDraftKind("host_tools_only", allowed))
    assertFalse(admitDraftKind("instruction_only", allowed))
  }

  @Test
  fun dirtyDraftImmediatelyInvalidatesValidation() {
    val revision = skillDetail(activeRevisionId = "revision-active").editableDraft!!
    val validated = SkillDraftContentState(
      revision = revision,
      instructions = revision.instructions,
      requiredTools = revision.requirements.runtimeTools,
      validation = validValidation(revision.revisionId, "hash-one"),
    )

    val dirty = draftInstructionsChanged(validated, "Changed after validation")

    assertEquals(null, dirty.validation)
    assertTrue(dirty.dirty)
    assertFalse(dirty.canActivate)
  }

  @Test
  fun saveAndRouteReentryCannotReuseStaleValidation() {
    val revision = skillDetail(activeRevisionId = "revision-active").editableDraft!!
    val state = SkillDraftContentState(
      revision = revision,
      instructions = revision.instructions,
      requiredTools = revision.requirements.runtimeTools,
      validation = validValidation(revision.revisionId, "old-hash"),
    )

    assertEquals(null, draftSaveStarted(state).validation)
    assertEquals(null, draftContentState(revision).validation)
  }

  @Test
  fun validationPlanPersistsCurrentBytesBeforeValidation() {
    val detail = skillDetail(activeRevisionId = "revision-active")
    val revision = detail.editableDraft!!
    val state = SkillDraftContentState(
      revision = revision,
      instructions = "Current visible bytes",
      requiredTools = listOf("host/search", "host/read"),
    )

    val plan = draftValidationPlan(detail, state)

    assertEquals(revision.revisionId, plan.revisionId)
    assertEquals(listOf("SKILL.md", "general-agent.json"), plan.files.map { it.path })
    assertEquals("Current visible bytes", plan.files.first().content)
  }

  @Test
  fun rollbackSelectionRejectsStagingAndTracksExplicitHistoryTarget() {
    val detail = skillDetailWithHistory()
    val staging = detail.revisions.first { it.status == "staging" }
    val old = detail.revisions.first { it.revisionId == "revision-old" }

    assertEquals(null, selectRollbackTarget(detail, staging))
    assertEquals(old, selectRollbackTarget(detail, old))
  }

  private fun validValidation(revisionId: String, hash: String) = RuntimeSkillValidation(
    ok = true,
    errors = emptyList(),
    warnings = emptyList(),
    requiredTools = listOf("host/search"),
    requiredConnectors = emptyList(),
    dependencies = emptyList(),
    requiredCapabilities = listOf("network.http"),
    resolverStatus = "active",
    resolverErrors = emptyList(),
    permissionDiffJson = "{}",
    revisionId = revisionId,
    contentHash = hash,
    snapshotGeneration = 7,
  )

  private fun skillDetailWithHistory(): RuntimeSkillDetail {
    val base = skillDetail(activeRevisionId = "revision-active")
    val active = base.editableDraft!!.copy(
      revisionId = "revision-active",
      status = "managed",
      editable = false,
      validation = RuntimeSkillValidationSummary(true, emptyList(), emptyList()),
    )
    val old = active.copy(revisionId = "revision-old", version = "0.9.0")
    return base.copy(revisions = listOf(base.editableDraft!!, active, old))
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
