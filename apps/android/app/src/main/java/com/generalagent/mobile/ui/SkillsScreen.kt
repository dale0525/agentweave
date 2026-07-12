package com.generalagent.mobile.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.Add
import androidx.compose.material.icons.outlined.Build
import androidx.compose.material.icons.outlined.ChevronRight
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material.icons.outlined.Extension
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.generalagent.mobile.runtime.RuntimeDiagnostics
import com.generalagent.mobile.runtime.RuntimeSkill
import com.generalagent.mobile.runtime.RuntimeSkillApproval
import com.generalagent.mobile.runtime.RuntimeSkillDetail
import com.generalagent.mobile.runtime.RuntimeSkillDraftFile
import com.generalagent.mobile.runtime.RuntimeSkillDraftRequest
import com.generalagent.mobile.runtime.RuntimeSkillRevision
import com.generalagent.mobile.runtime.RuntimeSkillRollbackOutcome
import com.generalagent.mobile.runtime.RuntimeSkillValidation
import com.generalagent.mobile.runtime.RuntimeClient
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

fun draftUpdateFiles(
  detail: RuntimeSkillDetail,
  revision: RuntimeSkillRevision,
  instructions: String,
  requiredTools: List<String>,
): List<RuntimeSkillDraftFile> {
  val requires = JSONObject()
    .put("packages", JSONArray(revision.requirements.packages))
    .put("capabilities", JSONArray(revision.requirements.capabilities))
    .put("runtimeTools", JSONArray(requiredTools))
    .put("connectors", JSONArray(revision.requirements.connectors))
  val descriptor = JSONObject()
    .put("schemaVersion", 1)
    .put("id", detail.packageId)
    .put("version", revision.version)
    .put("displayName", detail.displayName)
    .put("kind", revision.kind)
    .put("package", JSONObject().put("includeInstructions", true).put("includeRuntime", false))
    .put("compatibility", JSONObject().put("minimumRuntimeVersion", JSONObject.NULL).put("platforms", JSONArray()))
    .put("requires", requires)
  return listOf(
    RuntimeSkillDraftFile("SKILL.md", instructions),
    RuntimeSkillDraftFile("general-agent.json", descriptor.toString(2) + "\n"),
  )
}

fun permissionChanges(permissionDiffJson: String): List<String> =
  runCatching {
    val diff = JSONObject(permissionDiffJson)
    buildList {
      addPermissionChanges(diff, "addedCapabilities", "+ capability: ")
      addPermissionChanges(diff, "removedCapabilities", "- capability: ")
      addPermissionChanges(diff, "addedTools", "+ tool: ")
      addPermissionChanges(diff, "removedTools", "- tool: ")
      addPermissionChanges(diff, "addedConnectors", "+ connector: ")
      addPermissionChanges(diff, "removedConnectors", "- connector: ")
    }
  }.getOrDefault(emptyList())

private fun MutableList<String>.addPermissionChanges(
  diff: JSONObject,
  field: String,
  prefix: String,
) {
  val values = diff.optJSONArray(field) ?: return
  repeat(values.length()) { index -> add(prefix + values.getString(index)) }
}

internal enum class SkillApprovalOperation { Activation, Rollback, Removal }

internal data class SkillApprovalUiState(
  val operation: SkillApprovalOperation,
  val approval: RuntimeSkillApproval,
  val detail: RuntimeSkillDetail,
  val revision: RuntimeSkillRevision,
)

@Composable
fun SkillsScreen(
  mode: SkillScreenMode,
  actions: Set<SkillAction>,
  allowedKinds: Set<String>,
  protectedPackages: Set<String>,
  allowedOverrides: Set<String>,
  agentAuthoring: Boolean,
  canOverrideProtected: Boolean,
  inventory: List<RuntimeSkill>,
  diagnostics: RuntimeDiagnostics,
  initialError: String?,
  runtimeClient: RuntimeClient,
  onSnapshotChanged: (List<RuntimeSkill>, RuntimeDiagnostics) -> Unit,
  onImmersiveChanged: (Boolean) -> Unit,
  onBack: () -> Unit,
) {
  val scope = rememberCoroutineScope()
  var route by remember { mutableStateOf<SkillRoute>(SkillRoute.ListRoute) }
  val operationGeneration = remember { SkillOperationGeneration() }
  var state by remember {
    mutableStateOf(SkillManagementUiState(inventory, diagnostics, inlineError = initialError))
  }
  var draftValidation by remember { mutableStateOf<RuntimeSkillValidation?>(null) }
  var approval by remember { mutableStateOf<SkillApprovalUiState?>(null) }
  var synchronizationRecoveryPending by remember { mutableStateOf(false) }

  LaunchedEffect(inventory, diagnostics, initialError) {
    state = parentSkillSnapshotUpdated(state, inventory, diagnostics, initialError)
  }
  LaunchedEffect(route) {
    onImmersiveChanged(route !is SkillRoute.ListRoute)
  }

  fun fail(error: Throwable, fallback: String) {
    if (error is CancellationException) throw error
    state = skillOperationFailed(state, error.message ?: fallback)
  }

  fun navigate(next: SkillRoute) {
    operationGeneration.invalidate()
    route = next
    draftValidation = null
  }

  fun openDetail(packageId: String) {
    val generation = operationGeneration.begin()
    val inventorySkill = state.inventory.find { it.packageId == packageId }
    state = state.copy(busyOperation = "detail", inlineError = null)
    scope.launch {
      try {
        val detail = withContext(Dispatchers.IO) {
          skillDetailWithInventoryFacts(runtimeClient.getSkillDetail(packageId), inventorySkill)
        }
        if (!operationGeneration.accepts(generation)) return@launch
        state = state.copy(detail = detail, busyOperation = null)
        draftValidation = null
        route = SkillRoute.Detail(packageId)
      } catch (error: Throwable) {
        if (!operationGeneration.accepts(generation)) return@launch
        fail(error, "Unable to load skill detail")
      }
    }
  }

  fun refresh(returnToDetail: Boolean = false) {
    state = state.copy(busyOperation = "refresh", inlineError = null)
    val generation = operationGeneration.begin()
    scope.launch {
      try {
        val packageId = state.detail?.packageId
        val result = withContext(Dispatchers.IO) {
          val effective = runtimeClient.listSkills()
          val skills = if (mode == SkillScreenMode.OwnerManage) {
            ownerSkillInventory(effective, runtimeClient.listManagedSkills())
          } else {
            effective
          }
          val nextDiagnostics = runtimeClient.diagnostics()
          val detail = if (
            returnToDetail && packageId != null && skills.any { it.packageId == packageId }
          ) {
            skillDetailWithInventoryFacts(
              runtimeClient.getSkillDetail(packageId),
              skills.find { it.packageId == packageId },
            )
          } else {
            null
          }
          Triple(skills, nextDiagnostics, detail)
        }
        if (!operationGeneration.accepts(generation)) return@launch
        if (returnToDetail && result.third == null) {
          navigate(SkillRoute.ListRoute)
        }
        state = state.copy(
          inventory = result.first,
          diagnostics = result.second,
          detail = if (returnToDetail) result.third else state.detail,
          busyOperation = null,
        )
        onSnapshotChanged(result.first, result.second)
      } catch (error: Throwable) {
        if (!operationGeneration.accepts(generation)) return@launch
        fail(error, "Unable to refresh skills")
      }
    }
  }

  fun synchronizationRetryAction(returnToDetail: Boolean): () -> Unit {
    var generation = 0L
    return skillSynchronizationRetryCallback(
      scope = scope,
      onStart = {
        state = state.copy(busyOperation = "synchronize")
        generation = operationGeneration.begin()
      },
      synchronize = { withContext(Dispatchers.IO) { runtimeClient.synchronizeSkills() } },
      refresh = {
        if (operationGeneration.accepts(generation)) {
          synchronizationRecoveryPending = false
          refresh(returnToDetail)
        }
      },
      onFailure = { error ->
        if (operationGeneration.accepts(generation)) {
          state = publicationSynchronizationRetryFailed(
            state,
            error.message ?: "Requester synchronization failed",
          )
        }
      },
    )
  }

  fun requestApproval(operation: SkillApprovalOperation, revision: RuntimeSkillRevision) {
    val detail = state.detail ?: return
    state = state.copy(busyOperation = operation.name.lowercase(), inlineError = null)
    val generation = operationGeneration.begin()
    scope.launch {
      try {
        val requested = withContext(Dispatchers.IO) {
          when (operation) {
            SkillApprovalOperation.Activation -> runtimeClient.requestSkillActivation(revision.revisionId)
            SkillApprovalOperation.Removal -> runtimeClient.requestSkillRemoval(detail.packageId)
            SkillApprovalOperation.Rollback -> error("Rollback uses its dedicated flow")
          }
        }
        if (!operationGeneration.accepts(generation)) return@launch
        state = state.copy(busyOperation = null)
        approval = SkillApprovalUiState(operation, requested, detail, revision)
      } catch (error: Throwable) {
        if (!operationGeneration.accepts(generation)) return@launch
        fail(error, "Unable to request approval")
      }
    }
  }

  fun rollback(revision: RuntimeSkillRevision) {
    val detail = state.detail ?: return
    state = state.copy(busyOperation = "rollback", inlineError = null)
    val generation = operationGeneration.begin()
    scope.launch {
      try {
        val result = withContext(Dispatchers.IO) {
          runtimeClient.rollbackManagedSkill(detail.packageId, revision.revisionId)
        }
        if (!operationGeneration.accepts(generation)) return@launch
        when (result) {
          is RuntimeSkillRollbackOutcome.ApprovalRequired -> {
            approval = SkillApprovalUiState(
              SkillApprovalOperation.Rollback,
              result.approval,
              detail,
              revision,
            )
            state = state.copy(busyOperation = null)
          }
          is RuntimeSkillRollbackOutcome.Published -> refresh(returnToDetail = true)
        }
      } catch (error: Throwable) {
        if (!operationGeneration.accepts(generation)) return@launch
        fail(error, "Rollback failed")
      }
    }
  }

  fun resolveApproval() {
    val pending = approval ?: return
    state = state.copy(busyOperation = "approval", inlineError = null)
    scope.launch {
      try {
        val resolution = withContext(Dispatchers.IO) {
          runtimeClient.resolveSkillApproval(pending.approval.approvalId, approve = true)
        }
        approval = null
        val warning = resolution.synchronizationWarning
        if (pending.operation == SkillApprovalOperation.Removal) {
          navigate(SkillRoute.ListRoute)
          state = state.copy(detail = null)
        } else {
          navigate(SkillRoute.Detail(pending.detail.packageId))
        }
        if (warning != null) {
          synchronizationRecoveryPending = true
          state = state.copy(
            busyOperation = null,
            inlineError = publicationSynchronizationWarning(warning),
          )
        } else if (pending.operation == SkillApprovalOperation.Removal) {
          synchronizationRecoveryPending = false
          refresh()
        } else {
          synchronizationRecoveryPending = false
          refresh(returnToDetail = true)
        }
      } catch (error: Throwable) {
        approval = null
        fail(error, "Approval failed")
      }
    }
  }

  BackHandler {
    when (route) {
      SkillRoute.ListRoute -> onBack()
      is SkillRoute.Detail -> {
        navigate(SkillRoute.ListRoute)
        state = state.copy(detail = null, inlineError = null)
      }
      is SkillRoute.Draft -> navigate(if (state.detail == null) {
        SkillRoute.ListRoute
      } else {
        SkillRoute.Detail(checkNotNull(state.detail).packageId)
      })
    }
  }

  when (val currentRoute = route) {
    SkillRoute.ListRoute -> SkillInventoryRoute(
      mode = mode,
      actions = actions,
      state = state,
      onBack = onBack,
      onRefresh = if (synchronizationRecoveryPending) {
        synchronizationRetryAction(returnToDetail = false)
      } else {
        { refresh() }
      },
      onCreate = { navigate(SkillRoute.Draft(packageId = null, creating = true)) },
      onSelect = { skill -> openDetail(skill.packageId) },
    )
    is SkillRoute.Detail -> state.detail?.let { detail ->
      val targetActions = skillTargetActions(
        SkillAccessState(
          mode = mode,
          visibleTabs = emptyList(),
          actions = actions,
          allowedKinds = allowedKinds,
          protectedPackages = protectedPackages,
          allowedOverrides = allowedOverrides,
          agentAuthoring = agentAuthoring,
          canOverrideProtected = canOverrideProtected,
        ),
        detail,
        manageable = state.inventory.find { it.packageId == detail.packageId }?.manageable == true,
      )
      SkillDetailScreen(
        detail = detail,
        actions = targetActions,
        busyOperation = state.busyOperation,
        inlineError = state.inlineError,
        onRetry = if (synchronizationRecoveryPending) {
          synchronizationRetryAction(returnToDetail = true)
        } else {
          { refresh(returnToDetail = true) }
        },
        onBack = {
          navigate(SkillRoute.ListRoute)
          state = state.copy(detail = null, inlineError = null)
        },
        onEdit = { navigate(SkillRoute.Draft(detail.packageId, creating = false)) },
        onActivate = { revision -> requestApproval(SkillApprovalOperation.Activation, revision) },
        onDisable = {
          state = state.copy(busyOperation = "disable", inlineError = null)
          val generation = operationGeneration.begin()
          scope.launch {
            try {
              withContext(Dispatchers.IO) { runtimeClient.disableManagedSkill(detail.packageId) }
              if (!operationGeneration.accepts(generation)) return@launch
              refresh(returnToDetail = true)
            } catch (error: Throwable) {
              if (!operationGeneration.accepts(generation)) return@launch
              fail(error, "Disable failed")
            }
          }
        },
        onRollback = ::rollback,
        onRemove = { revision -> requestApproval(SkillApprovalOperation.Removal, revision) },
      )
    }
    is SkillRoute.Draft -> SkillDraftRoute(
      creating = currentRoute.creating,
      detail = state.detail,
      actions = actions,
      allowedKinds = allowedKinds,
      busyOperation = state.busyOperation,
      externalError = state.inlineError,
      validation = draftValidation,
      runtimeClient = runtimeClient,
      onBusy = { operation -> state = state.copy(busyOperation = operation, inlineError = null) },
      onFailure = { error, fallback -> fail(error, fallback) },
      onSaved = { detail ->
        val inventorySkill = state.inventory.find { it.packageId == detail.packageId }
        state = state.copy(
          detail = skillDetailWithInventoryFacts(detail, inventorySkill),
          busyOperation = null,
          inlineError = null,
        )
        navigate(SkillRoute.Detail(detail.packageId))
        refresh(returnToDetail = true)
      },
      onValidated = { validation, savedInstructions, savedTools ->
        draftValidation = validation
        val detail = state.detail
        val draft = detail?.editableDraft
        val merged = if (draft != null && validation.revisionId == draft.revisionId) {
          revisionAfterValidation(draft, validation, savedInstructions, savedTools)
        } else {
          null
        }
        state = state.copy(
          detail = if (detail != null && merged != null) {
            detail.copy(
              editableDraft = merged,
              revisions = detail.revisions.map { if (it.revisionId == merged.revisionId) merged else it },
            )
          } else {
            detail
          },
          busyOperation = null,
          inlineError = null,
        )
      },
      onDraftChanged = { draftValidation = null },
      onActivate = { revision -> requestApproval(SkillApprovalOperation.Activation, revision) },
      onBack = {
        navigate(state.detail?.let { SkillRoute.Detail(it.packageId) } ?: SkillRoute.ListRoute)
        state = state.copy(inlineError = null)
      },
    )
  }

  approval?.let { pending ->
    SkillApprovalDialog(
      state = pending,
      busy = state.busyOperation == "approval",
      approvingActor = runtimeClient.approverActorId,
      approvalAvailable = runtimeClient.approvalAvailable,
      approvalUnavailableReason = runtimeClient.approvalUnavailableReason,
      onDismiss = { if (state.busyOperation != "approval") approval = null },
      onConfirm = ::resolveApproval,
    )
  }
}

@Composable
internal fun SourceLabel(source: String) {
  Text(
    text = if (source == "managed") "MANAGED" else "BUILT-IN",
    color = if (source == "managed") GaPrimaryActive else GaTextSecondary,
    style = MaterialTheme.typography.labelMedium,
    modifier = Modifier
      .background(if (source == "managed") Color(0xFFD9F3EF) else GaSurfaceMuted, GaSmallShape)
      .padding(horizontal = 6.dp, vertical = 3.dp),
  )
}

@Composable
internal fun InlineSkillError(message: String, onRetry: (() -> Unit)? = null) {
  Row(
    modifier = Modifier
      .fillMaxWidth()
      .background(MaterialTheme.colorScheme.errorContainer)
      .semantics {
        liveRegion = LiveRegionMode.Assertive
        contentDescription = message
      }
      .padding(horizontal = 16.dp, vertical = 10.dp),
    verticalAlignment = Alignment.CenterVertically,
  ) {
    Icon(
      Icons.Outlined.ErrorOutline,
      contentDescription = null,
      tint = MaterialTheme.colorScheme.error,
      modifier = Modifier.size(20.dp),
    )
    Text(
      message,
      color = MaterialTheme.colorScheme.error,
      style = MaterialTheme.typography.bodyMedium,
      modifier = Modifier.weight(1f).padding(start = 10.dp),
      maxLines = 3,
      overflow = TextOverflow.Ellipsis,
    )
    if (onRetry != null) {
      TextButton(onClick = onRetry) { Text("Retry") }
    }
  }
}

internal fun statusLabel(value: String): String =
  value.replace('_', ' ').replaceFirstChar { it.uppercase() }

internal fun shortRevision(value: String): String =
  if (value.length <= 12) value else value.take(8) + "…" + value.takeLast(4)
