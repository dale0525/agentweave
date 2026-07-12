package com.generalagent.mobile.ui

import com.generalagent.mobile.runtime.RuntimeActorContext
import com.generalagent.mobile.runtime.RuntimeDiagnostics
import com.generalagent.mobile.runtime.RuntimeSkill
import com.generalagent.mobile.runtime.RuntimeSkillDetail
import com.generalagent.mobile.runtime.RuntimeSkillDraftFile
import com.generalagent.mobile.runtime.RuntimeSkillPolicy
import com.generalagent.mobile.runtime.RuntimeSkillRevision
import com.generalagent.mobile.runtime.RuntimeSkillValidation
import org.json.JSONArray
import org.json.JSONObject

sealed interface SkillRoute {
  data object ListRoute : SkillRoute
  data class Detail(val packageId: String) : SkillRoute
  data class Draft(val packageId: String?, val creating: Boolean) : SkillRoute
}

data class SkillManagementUiState(
  val inventory: List<RuntimeSkill>,
  val diagnostics: RuntimeDiagnostics,
  val detail: RuntimeSkillDetail? = null,
  val busyOperation: String? = null,
  val inlineError: String? = null,
)

fun skillOperationFailed(state: SkillManagementUiState, message: String): SkillManagementUiState =
  state.copy(busyOperation = null, inlineError = message.trim().ifEmpty { "Skill operation failed" })

fun skillAccessState(policy: RuntimeSkillPolicy, actor: RuntimeActorContext): SkillAccessState {
  val mode = when {
    policy.mode == "disabled" -> SkillScreenMode.Hidden
    policy.mode == "diagnostics_only" -> SkillScreenMode.DiagnosticsOnly
    policy.mode == "owner_only" && actor.role == "owner" && "inspect" in actor.grants ->
      SkillScreenMode.OwnerManage
    else -> SkillScreenMode.Hidden
  }
  val tabs = visibleTabs(policy.mode).filter { it != AppTab.Skills || mode != SkillScreenMode.Hidden }
  val granted = skillActions(mode, actor.grants.toSet()).toMutableSet()
  val allowedKinds = policy.allowedKinds.toSet()
  if (!policy.agentAuthoring || allowedKinds.isEmpty()) granted.remove(SkillAction.Create)
  return SkillAccessState(
    mode = mode,
    visibleTabs = tabs,
    actions = granted,
    allowedKinds = allowedKinds,
    protectedPackages = policy.protectedPackages.toSet(),
    allowedOverrides = policy.allowedOverrides.toSet(),
    agentAuthoring = policy.agentAuthoring,
  )
}

fun skillTargetActions(
  access: SkillAccessState,
  detail: RuntimeSkillDetail,
  manageable: Boolean,
): Set<SkillAction> {
  if (access.mode != SkillScreenMode.OwnerManage || detail.sourceLayer != "managed") return emptySet()
  if (!manageable || detail.packageId in access.protectedPackages) return emptySet()
  if (detail.status in setOf("removed", "quarantined", "inactive")) return emptySet()
  val draft = detail.editableDraft
  val draftAllowed = draft != null && draft.kind in access.allowedKinds
  val active = detail.activeRevisionId?.let { activeId ->
    detail.revisions.find { it.revisionId == activeId && !it.editable && it.status == "managed" }
  }
  val activeInstallation = detail.status == "active" && active != null
  val activeKindAllowed = active?.kind in access.allowedKinds
  val rollbackAvailable = activeInstallation && detail.revisions.any { revision ->
    revision.kind in access.allowedKinds && selectRollbackTarget(detail, revision) != null
  }
  return access.actions.filterTo(mutableSetOf()) { action ->
    when (action) {
      SkillAction.Create -> false
      SkillAction.Edit, SkillAction.Validate, SkillAction.Activate -> draftAllowed
      SkillAction.Disable -> activeInstallation && activeKindAllowed
      SkillAction.Rollback -> rollbackAvailable && activeKindAllowed
      SkillAction.Delete -> activeInstallation && activeKindAllowed
    }
  }
}

fun initialDraftKind(allowedKinds: Set<String>): String = allowedKinds.firstOrNull().orEmpty()

fun admitDraftKind(kind: String, allowedKinds: Set<String>): Boolean = kind in allowedKinds

fun parentSkillSnapshotUpdated(
  state: SkillManagementUiState,
  inventory: List<RuntimeSkill>,
  diagnostics: RuntimeDiagnostics,
  initialError: String?,
): SkillManagementUiState =
  state.copy(
    inventory = inventory,
    diagnostics = diagnostics,
    inlineError = initialError ?: state.inlineError,
  )

class SkillOperationGeneration {
  private var generation = 0L

  fun begin(): Long {
    generation += 1
    return generation
  }

  fun invalidate() {
    generation += 1
  }

  fun accepts(token: Long): Boolean = token == generation
}

data class SkillDraftContentState(
  val revision: RuntimeSkillRevision,
  val instructions: String,
  val requiredTools: List<String>,
  val validation: RuntimeSkillValidation? = null,
  val dirty: Boolean = false,
) {
  val canActivate: Boolean
    get() = !dirty && validation?.ok == true && validation.revisionId == revision.revisionId
}

data class SkillDraftValidationPlan(
  val revisionId: String,
  val files: List<RuntimeSkillDraftFile>,
)

fun draftContentState(revision: RuntimeSkillRevision): SkillDraftContentState =
  SkillDraftContentState(
    revision = revision,
    instructions = revision.instructions,
    requiredTools = revision.requirements.runtimeTools,
  )

fun draftInstructionsChanged(state: SkillDraftContentState, instructions: String): SkillDraftContentState =
  state.copy(instructions = instructions, validation = null, dirty = true)

fun draftToolsChanged(state: SkillDraftContentState, tools: List<String>): SkillDraftContentState =
  state.copy(requiredTools = tools, validation = null, dirty = true)

fun draftSaveStarted(state: SkillDraftContentState): SkillDraftContentState =
  state.copy(validation = null)

fun draftValidationPlan(
  detail: RuntimeSkillDetail,
  state: SkillDraftContentState,
): SkillDraftValidationPlan =
  SkillDraftValidationPlan(
    revisionId = state.revision.revisionId,
    files = draftUpdateFiles(detail, state.revision, state.instructions, state.requiredTools),
  )

fun initialDraftFiles(
  packageId: String,
  displayName: String,
  kind: String,
  instructions: String,
  requiredTools: List<String>,
): List<RuntimeSkillDraftFile> {
  val descriptor = JSONObject()
    .put("schemaVersion", 1)
    .put("id", packageId)
    .put("version", "0.1.0")
    .put("displayName", displayName)
    .put("kind", kind)
    .put("package", JSONObject().put("includeInstructions", true).put("includeRuntime", false))
    .put("compatibility", JSONObject().put("minimumRuntimeVersion", JSONObject.NULL).put("platforms", JSONArray()))
    .put(
      "requires",
      JSONObject()
        .put("packages", JSONArray())
        .put("capabilities", JSONArray())
        .put("runtimeTools", JSONArray(requiredTools))
        .put("connectors", JSONArray()),
    )
  return listOf(
    RuntimeSkillDraftFile("SKILL.md", instructions),
    RuntimeSkillDraftFile("general-agent.json", descriptor.toString(2) + "\n"),
  )
}

fun selectRollbackTarget(
  detail: RuntimeSkillDetail,
  revision: RuntimeSkillRevision,
): RuntimeSkillRevision? =
  revision.takeIf {
    detail.sourceLayer == "managed" &&
      it in detail.revisions &&
      !it.editable &&
      it.status == "managed" &&
      it.revisionId != detail.activeRevisionId
  }
