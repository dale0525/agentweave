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
import com.generalagent.mobile.runtime.RuntimeSkillMutation
import com.generalagent.mobile.runtime.RuntimeSkillRevision
import com.generalagent.mobile.runtime.RuntimeSkillValidation
import com.generalagent.mobile.runtime.RuntimeClient
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
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

fun approvalCapabilities(permissionDiffJson: String, fallback: List<String>): List<String> =
  runCatching {
    val added = JSONObject(permissionDiffJson)
      .optJSONObject("capabilities")
      ?.optJSONArray("added")
      ?: return@runCatching fallback
    List(added.length()) { index -> added.getString(index) }
  }.getOrDefault(fallback)

private sealed interface SkillRoute {
  data object ListRoute : SkillRoute
  data object DetailRoute : SkillRoute
  data class DraftRoute(val creating: Boolean) : SkillRoute
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
  inventory: List<RuntimeSkill>,
  diagnostics: RuntimeDiagnostics,
  runtimeClient: RuntimeClient,
  onSnapshotChanged: (List<RuntimeSkill>, RuntimeDiagnostics) -> Unit,
  onImmersiveChanged: (Boolean) -> Unit,
  onBack: () -> Unit,
) {
  val scope = rememberCoroutineScope()
  var route by remember { mutableStateOf<SkillRoute>(SkillRoute.ListRoute) }
  var state by remember(inventory, diagnostics) {
    mutableStateOf(SkillManagementUiState(inventory, diagnostics))
  }
  var draftValidation by remember { mutableStateOf<RuntimeSkillValidation?>(null) }
  var approval by remember { mutableStateOf<SkillApprovalUiState?>(null) }

  LaunchedEffect(inventory, diagnostics) {
    state = state.copy(inventory = inventory, diagnostics = diagnostics)
  }
  LaunchedEffect(route) {
    onImmersiveChanged(route !is SkillRoute.ListRoute)
  }

  fun fail(error: Throwable, fallback: String) {
    if (error is CancellationException) throw error
    state = skillOperationFailed(state, error.message ?: fallback)
  }

  fun openDetail(packageId: String) {
    state = state.copy(busyOperation = "detail", inlineError = null)
    scope.launch {
      try {
        val detail = withContext(Dispatchers.IO) { runtimeClient.getSkillDetail(packageId) }
        state = state.copy(detail = detail, busyOperation = null)
        draftValidation = null
        route = SkillRoute.DetailRoute
      } catch (error: Throwable) {
        fail(error, "Unable to load skill detail")
      }
    }
  }

  fun refresh(returnToDetail: Boolean = false) {
    state = state.copy(busyOperation = "refresh", inlineError = null)
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
          val detail = if (returnToDetail && packageId != null) {
            runtimeClient.getSkillDetail(packageId)
          } else {
            null
          }
          Triple(skills, nextDiagnostics, detail)
        }
        state = state.copy(
          inventory = result.first,
          diagnostics = result.second,
          detail = result.third ?: state.detail,
          busyOperation = null,
        )
        onSnapshotChanged(result.first, result.second)
      } catch (error: Throwable) {
        fail(error, "Unable to refresh skills")
      }
    }
  }

  fun requestApproval(operation: SkillApprovalOperation, revision: RuntimeSkillRevision) {
    val detail = state.detail ?: return
    state = state.copy(busyOperation = operation.name.lowercase(), inlineError = null)
    scope.launch {
      try {
        val requested = withContext(Dispatchers.IO) {
          when (operation) {
            SkillApprovalOperation.Activation -> runtimeClient.requestSkillActivation(revision.revisionId)
            SkillApprovalOperation.Removal -> runtimeClient.requestSkillRemoval(detail.packageId)
            SkillApprovalOperation.Rollback -> error("Rollback uses its dedicated flow")
          }
        }
        state = state.copy(busyOperation = null)
        approval = SkillApprovalUiState(operation, requested, detail, revision)
      } catch (error: Throwable) {
        fail(error, "Unable to request approval")
      }
    }
  }

  fun rollback(revision: RuntimeSkillRevision) {
    val detail = state.detail ?: return
    state = state.copy(busyOperation = "rollback", inlineError = null)
    scope.launch {
      try {
        val result = withContext(Dispatchers.IO) {
          runtimeClient.rollbackManagedSkill(detail.packageId, revision.revisionId)
        }
        if (result.approvalRequired) {
          approval = SkillApprovalUiState(
            SkillApprovalOperation.Rollback,
            result.toApproval(detail.packageId, revision.revisionId),
            detail,
            revision,
          )
          state = state.copy(busyOperation = null)
        } else {
          refresh(returnToDetail = true)
        }
      } catch (error: Throwable) {
        fail(error, "Rollback failed")
      }
    }
  }

  fun resolveApproval() {
    val pending = approval ?: return
    state = state.copy(busyOperation = "approval", inlineError = null)
    scope.launch {
      try {
        withContext(Dispatchers.IO) {
          runtimeClient.resolveSkillApproval(pending.approval.approvalId, approve = true)
        }
        approval = null
        if (pending.operation == SkillApprovalOperation.Removal) {
          route = SkillRoute.ListRoute
          state = state.copy(detail = null)
          refresh()
        } else {
          route = SkillRoute.DetailRoute
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
      SkillRoute.DetailRoute -> {
        route = SkillRoute.ListRoute
        state = state.copy(detail = null, inlineError = null)
      }
      is SkillRoute.DraftRoute -> route = if (state.detail == null) {
        SkillRoute.ListRoute
      } else {
        SkillRoute.DetailRoute
      }
    }
  }

  when (val currentRoute = route) {
    SkillRoute.ListRoute -> SkillInventoryRoute(
      mode = mode,
      actions = actions,
      state = state,
      onBack = onBack,
      onRefresh = { refresh() },
      onCreate = { route = SkillRoute.DraftRoute(creating = true) },
      onSelect = { skill -> openDetail(skill.packageId) },
    )
    SkillRoute.DetailRoute -> state.detail?.let { detail ->
      SkillDetailScreen(
        detail = detail,
        actions = actions,
        busyOperation = state.busyOperation,
        inlineError = state.inlineError,
        onBack = {
          route = SkillRoute.ListRoute
          state = state.copy(detail = null, inlineError = null)
        },
        onEdit = { route = SkillRoute.DraftRoute(creating = false) },
        onActivate = { revision -> requestApproval(SkillApprovalOperation.Activation, revision) },
        onDisable = {
          state = state.copy(busyOperation = "disable", inlineError = null)
          scope.launch {
            try {
              withContext(Dispatchers.IO) { runtimeClient.disableManagedSkill(detail.packageId) }
              refresh(returnToDetail = true)
            } catch (error: Throwable) {
              fail(error, "Disable failed")
            }
          }
        },
        onRollback = ::rollback,
        onRemove = { revision -> requestApproval(SkillApprovalOperation.Removal, revision) },
      )
    }
    is SkillRoute.DraftRoute -> SkillDraftRoute(
      creating = currentRoute.creating,
      detail = state.detail,
      actions = actions,
      busyOperation = state.busyOperation,
      externalError = state.inlineError,
      validation = draftValidation,
      runtimeClient = runtimeClient,
      onBusy = { operation -> state = state.copy(busyOperation = operation, inlineError = null) },
      onFailure = { error, fallback -> fail(error, fallback) },
      onSaved = { detail ->
        state = state.copy(detail = detail, busyOperation = null, inlineError = null)
        route = SkillRoute.DetailRoute
      },
      onValidated = { validation ->
        draftValidation = validation
        state = state.copy(busyOperation = null, inlineError = null)
      },
      onActivate = { revision -> requestApproval(SkillApprovalOperation.Activation, revision) },
      onBack = {
        route = if (state.detail == null) SkillRoute.ListRoute else SkillRoute.DetailRoute
        state = state.copy(inlineError = null)
      },
    )
  }

  approval?.let { pending ->
    SkillApprovalDialog(
      state = pending,
      busy = state.busyOperation == "approval",
      onDismiss = { if (state.busyOperation != "approval") approval = null },
      onConfirm = ::resolveApproval,
    )
  }
}

@Composable
private fun SkillInventoryRoute(
  mode: SkillScreenMode,
  actions: Set<SkillAction>,
  state: SkillManagementUiState,
  onBack: () -> Unit,
  onRefresh: () -> Unit,
  onCreate: () -> Unit,
  onSelect: (RuntimeSkill) -> Unit,
) {
  Column(modifier = Modifier.fillMaxSize().background(GaSurface)) {
    SkillInventoryTopBar(
      title = if (mode == SkillScreenMode.DiagnosticsOnly) "Skill diagnostics" else "Managed skills",
      busy = state.busyOperation == "refresh",
      onBack = onBack,
      onRefresh = onRefresh,
    )
    SnapshotStrip(state.diagnostics, state.inventory)
    if (mode == SkillScreenMode.OwnerManage && SkillAction.Create in actions) {
      Row(
        modifier = Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.End,
      ) {
        Button(onClick = onCreate, modifier = Modifier.height(40.dp)) {
          Icon(Icons.Outlined.Add, contentDescription = null, modifier = Modifier.size(18.dp))
          Spacer(Modifier.size(8.dp))
          Text("New draft")
        }
      }
      HorizontalDivider(color = GaBorder)
    }
    state.inlineError?.let { InlineSkillError(it) }
    if (state.inventory.isEmpty()) {
      EmptySkillInventory(mode)
    } else {
      LazyColumn(modifier = Modifier.fillMaxSize()) {
        items(state.inventory, key = { it.packageId }) { skill ->
          SkillInventoryRow(
            skill = skill,
            generation = state.diagnostics.activeSnapshotGeneration,
            interactive = mode == SkillScreenMode.OwnerManage,
            onClick = { onSelect(skill) },
          )
          HorizontalDivider(color = GaBorder, modifier = Modifier.padding(horizontal = 16.dp))
        }
        item { Spacer(Modifier.height(16.dp)) }
      }
    }
  }
}

@Composable
private fun SkillInventoryTopBar(
  title: String,
  busy: Boolean,
  onBack: () -> Unit,
  onRefresh: () -> Unit,
) {
  Row(
    modifier = Modifier.fillMaxWidth().height(64.dp).padding(horizontal = 8.dp),
    verticalAlignment = Alignment.CenterVertically,
  ) {
    IconButton(onClick = onBack, modifier = Modifier.size(48.dp)) {
      Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = "Back")
    }
    Text(
      text = title,
      style = MaterialTheme.typography.titleMedium,
      modifier = Modifier.weight(1f),
      maxLines = 1,
      overflow = TextOverflow.Ellipsis,
    )
    IconButton(
      onClick = onRefresh,
      enabled = !busy,
      modifier = Modifier.size(48.dp),
    ) {
      if (busy) {
        CircularProgressIndicator(modifier = Modifier.size(20.dp), strokeWidth = 2.dp)
      } else {
        Icon(Icons.Outlined.Refresh, contentDescription = "Refresh skill diagnostics")
      }
    }
  }
  HorizontalDivider(color = GaBorder)
}

@Composable
private fun SnapshotStrip(diagnostics: RuntimeDiagnostics, skills: List<RuntimeSkill>) {
  Row(
    modifier = Modifier
      .fillMaxWidth()
      .height(52.dp)
      .background(GaSurfaceSubtle)
      .padding(horizontal = 16.dp),
    verticalAlignment = Alignment.CenterVertically,
    horizontalArrangement = Arrangement.SpaceBetween,
  ) {
    Column {
      Text("SNAPSHOT", style = MaterialTheme.typography.labelMedium, color = GaTextSecondary)
      Text(
        "Generation ${diagnostics.activeSnapshotGeneration}",
        style = MaterialTheme.typography.bodyMedium,
        fontWeight = FontWeight.SemiBold,
      )
    }
    Text(
      "${skills.count { it.available }} active / ${skills.size} total",
      style = MaterialTheme.typography.labelMedium,
      color = GaTextSecondary,
    )
  }
  HorizontalDivider(color = GaBorder)
}

@Composable
private fun SkillInventoryRow(
  skill: RuntimeSkill,
  generation: Long,
  interactive: Boolean,
  onClick: () -> Unit,
) {
  val semantics = buildString {
    append(skill.displayName)
    append(", source ${skill.sourceLayer}, status ${skill.status}")
    skill.activeRevisionId?.let { append(", revision $it") }
    if (skill.reason.isNotBlank()) append(", ${skill.reason}")
    append(", generation $generation")
  }
  val rowModifier = Modifier
    .fillMaxWidth()
    .heightIn(min = 96.dp)
    .semantics { contentDescription = semantics }
    .then(if (interactive) Modifier.clickable(onClick = onClick) else Modifier)
    .padding(horizontal = 16.dp, vertical = 12.dp)
  Row(modifier = rowModifier, verticalAlignment = Alignment.Top) {
    Box(
      modifier = Modifier.size(40.dp).background(
        if (skill.available) GaReadyContainer else GaAmberContainer,
        GaSmallShape,
      ),
      contentAlignment = Alignment.Center,
    ) {
      Icon(
        if (skill.available) Icons.Outlined.Extension else Icons.Outlined.ErrorOutline,
        contentDescription = null,
        tint = if (skill.available) GaReady else GaAmber,
        modifier = Modifier.size(21.dp),
      )
    }
    Column(modifier = Modifier.weight(1f).padding(start = 12.dp, end = 8.dp)) {
      Row(verticalAlignment = Alignment.CenterVertically) {
        Text(
          skill.displayName,
          style = MaterialTheme.typography.bodyMedium,
          fontWeight = FontWeight.SemiBold,
          maxLines = 1,
          overflow = TextOverflow.Ellipsis,
          modifier = Modifier.weight(1f),
        )
        SourceLabel(skill.sourceLayer)
      }
      Text(
        "${skill.packageId} • ${statusLabel(skill.status)}",
        color = GaTextSecondary,
        fontSize = 12.sp,
        lineHeight = 18.sp,
        maxLines = 2,
        overflow = TextOverflow.Ellipsis,
      )
      Text(
        skill.activeRevisionId?.let { "Revision ${shortRevision(it)}" }
          ?: "No active revision",
        color = GaTextSecondary,
        fontSize = 12.sp,
        lineHeight = 18.sp,
      )
      if (skill.reason.isNotBlank()) {
        Text(
          skill.reason,
          color = if (skill.available) GaTextSecondary else GaAmberText,
          fontSize = 12.sp,
          lineHeight = 18.sp,
          maxLines = 2,
          overflow = TextOverflow.Ellipsis,
        )
      }
    }
    if (interactive) {
      Icon(
        Icons.Outlined.ChevronRight,
        contentDescription = "Open skill detail",
        tint = GaTextSecondary,
        modifier = Modifier.size(24.dp).padding(top = 8.dp),
      )
    }
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
internal fun InlineSkillError(message: String) {
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
      modifier = Modifier.padding(start = 10.dp),
      maxLines = 3,
      overflow = TextOverflow.Ellipsis,
    )
  }
}

@Composable
private fun EmptySkillInventory(mode: SkillScreenMode) {
  Column(
    modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 48.dp),
    horizontalAlignment = Alignment.CenterHorizontally,
    verticalArrangement = Arrangement.spacedBy(10.dp),
  ) {
    Icon(Icons.Outlined.Build, contentDescription = null, tint = GaTextSecondary)
    Text("No skill packages", fontWeight = FontWeight.SemiBold)
    Text(
      if (mode == SkillScreenMode.DiagnosticsOnly) {
        "The Android runtime did not report built-in or managed skills."
      } else {
        "Create a draft to begin managing an owner skill."
      },
      color = GaTextSecondary,
      style = MaterialTheme.typography.bodyMedium,
    )
  }
}

internal fun statusLabel(value: String): String =
  value.replace('_', ' ').replaceFirstChar { it.uppercase() }

internal fun shortRevision(value: String): String =
  if (value.length <= 12) value else value.take(8) + "…" + value.takeLast(4)

private fun RuntimeSkillMutation.toApproval(packageId: String, revisionId: String) = RuntimeSkillApproval(
  approvalId = checkNotNull(approvalId),
  packageId = this.packageId ?: packageId,
  permissionDiffJson = "{}",
  requestedBy = "current owner",
  revisionId = this.revisionId ?: revisionId,
  status = status,
)
