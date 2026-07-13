package com.generalagent.mobile.ui

import com.generalagent.mobile.runtime.RuntimeActorContext
import com.generalagent.mobile.runtime.RuntimeDiagnostics
import com.generalagent.mobile.runtime.RuntimeSkill
import com.generalagent.mobile.runtime.RuntimeSkillDetail
import com.generalagent.mobile.runtime.RuntimeSkillDraftFile
import com.generalagent.mobile.runtime.RuntimeSkillPolicy
import com.generalagent.mobile.runtime.RuntimeSkillRevision
import com.generalagent.mobile.runtime.RuntimeSkillValidation
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
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
    canOverrideProtected = "override_builtin" in actor.grants,
  )
}

fun skillTargetActions(
  access: SkillAccessState,
  detail: RuntimeSkillDetail,
): Set<SkillAction> {
  if (access.mode != SkillScreenMode.OwnerManage) return emptySet()
  return buildSet {
    val facts = detail.actions
    if (facts.canEditDraft) add(SkillAction.Edit)
    if (facts.canValidateDraft) add(SkillAction.Validate)
    if (facts.canRequestActivation) add(SkillAction.Activate)
    if (facts.canDisable) add(SkillAction.Disable)
    if (facts.canRollback) add(SkillAction.Rollback)
    if (facts.canRequestRemoval) add(SkillAction.Delete)
  }
}

fun skillDetailWithInventoryFacts(
  detail: RuntimeSkillDetail,
  inventorySkill: RuntimeSkill?,
): RuntimeSkillDetail =
  detail.copy(
    builtInCollision = detail.builtInCollision ||
      inventorySkill?.builtInCollision == true,
  )

private val existingDraftActionSet =
  setOf(SkillAction.Edit, SkillAction.Validate, SkillAction.Activate)

fun draftRouteActions(
  creating: Boolean,
  globalActions: Set<SkillAction>,
  targetActions: Set<SkillAction>,
): Set<SkillAction> =
  if (creating) globalActions else targetActions.intersect(existingDraftActionSet)

fun canOpenExistingDraft(actions: Set<SkillAction>): Boolean =
  actions.any(existingDraftActionSet::contains)

fun shouldPersistBeforeValidation(actions: Set<SkillAction>): Boolean =
  SkillAction.Edit in actions

fun authoritativeDraftActivationRevision(
  revision: RuntimeSkillRevision?,
  validation: RuntimeSkillValidation?,
  dirty: Boolean,
): RuntimeSkillRevision? {
  if (revision == null || dirty) return null
  if (validation != null) {
    return validation
      .takeIf { it.ok && it.revisionId == revision.revisionId }
      ?.let {
        revisionAfterValidation(
          revision,
          it,
          savedInstructions = revision.instructions,
          savedTools = revision.requirements.runtimeTools,
        )
      }
  }
  return revision.takeIf { it.validation.ok && it.contentHash.isNotBlank() }
}

fun createdDraftDetailWithCollision(
  detail: RuntimeSkillDetail,
  builtInCollision: Boolean,
): RuntimeSkillDetail =
  detail.copy(builtInCollision = detail.builtInCollision || builtInCollision)

internal fun skillSynchronizationRetryCallback(
  scope: CoroutineScope,
  onStart: () -> Unit = {},
  synchronize: suspend () -> Unit,
  refresh: () -> Unit,
  onFailure: (Throwable) -> Unit,
): () -> Unit = {
  onStart()
  scope.launch {
    try {
      synchronize()
      refresh()
    } catch (error: Throwable) {
      if (error is CancellationException) throw error
      onFailure(error)
    }
  }
}

fun publicationSynchronizationWarning(message: String): String =
  "Published, refresh required: $message"

fun publicationSynchronizationRetryFailed(
  state: SkillManagementUiState,
  message: String,
): SkillManagementUiState =
  skillOperationFailed(state, publicationSynchronizationWarning(message))

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
