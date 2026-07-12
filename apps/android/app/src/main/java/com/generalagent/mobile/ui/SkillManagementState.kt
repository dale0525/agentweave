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
  val allowedKinds = if (policy.agentAuthoring) policy.allowedKinds.toSet() else emptySet()
  if (allowedKinds.isEmpty()) granted.remove(SkillAction.Create)
  return SkillAccessState(mode, tabs, granted, allowedKinds)
}

fun skillTargetActions(access: SkillAccessState, detail: RuntimeSkillDetail): Set<SkillAction> {
  if (access.mode != SkillScreenMode.OwnerManage || detail.sourceLayer != "managed") return emptySet()
  if (detail.status in setOf("removed", "quarantined")) return emptySet()
  val draft = detail.editableDraft
  val draftAllowed = draft != null && draft.kind in access.allowedKinds
  return access.actions.filterTo(mutableSetOf()) { action ->
    when (action) {
      SkillAction.Create -> false
      SkillAction.Edit, SkillAction.Validate, SkillAction.Activate -> draftAllowed
      SkillAction.Disable -> detail.activeRevisionId != null && detail.status == "active"
      SkillAction.Rollback -> detail.revisions.any { selectRollbackTarget(detail, it) != null }
      SkillAction.Delete -> detail.activeRevisionId != null || draft != null
    }
  }
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
