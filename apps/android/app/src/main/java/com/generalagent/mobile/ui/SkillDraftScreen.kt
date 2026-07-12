package com.generalagent.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.ExpandMore
import androidx.compose.material.icons.outlined.Publish
import androidx.compose.material.icons.outlined.Save
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.generalagent.mobile.runtime.RuntimeClient
import com.generalagent.mobile.runtime.RuntimeSkillDetail
import com.generalagent.mobile.runtime.RuntimeSkillDraftRequest
import com.generalagent.mobile.runtime.RuntimeSkillRevision
import com.generalagent.mobile.runtime.RuntimeSkillRequirements
import com.generalagent.mobile.runtime.RuntimeSkillValidation
import com.generalagent.mobile.runtime.RuntimeSkillValidationSummary
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

fun revisionAfterValidation(
  revision: RuntimeSkillRevision,
  validation: RuntimeSkillValidation,
): RuntimeSkillRevision = revision.copy(
  validation = RuntimeSkillValidationSummary(
    ok = validation.ok,
    errors = validation.errors,
    warnings = validation.warnings,
  ),
  requirements = RuntimeSkillRequirements(
    runtimeTools = validation.requiredTools,
    capabilities = validation.requiredCapabilities,
    connectors = validation.requiredConnectors,
    packages = validation.dependencies,
  ),
  permissionDiffJson = validation.permissionDiffJson,
)

@Composable
internal fun SkillDraftRoute(
  creating: Boolean,
  detail: RuntimeSkillDetail?,
  actions: Set<SkillAction>,
  busyOperation: String?,
  externalError: String?,
  validation: RuntimeSkillValidation?,
  runtimeClient: RuntimeClient,
  onBusy: (String) -> Unit,
  onFailure: (Throwable, String) -> Unit,
  onSaved: (RuntimeSkillDetail) -> Unit,
  onValidated: (RuntimeSkillValidation) -> Unit,
  onActivate: (RuntimeSkillRevision) -> Unit,
  onBack: () -> Unit,
) {
  val scope = rememberCoroutineScope()
  val existingDraft = detail?.editableDraft
  var packageId by remember(detail, creating) { mutableStateOf(if (creating) "" else detail?.packageId.orEmpty()) }
  var displayName by remember(detail, creating) { mutableStateOf(if (creating) "" else detail?.displayName.orEmpty()) }
  var description by remember(detail, creating) { mutableStateOf("") }
  var kind by remember(existingDraft, creating) {
    mutableStateOf(if (creating) "instruction_only" else existingDraft?.kind ?: "instruction_only")
  }
  var instructions by remember(existingDraft, creating) { mutableStateOf(existingDraft?.instructions.orEmpty()) }
  var requiredTools by remember(existingDraft, creating) {
    mutableStateOf(existingDraft?.requirements?.runtimeTools?.joinToString(", ").orEmpty())
  }
  var localError by remember { mutableStateOf<String?>(null) }
  val busy = busyOperation != null
  val activationRevision = existingDraft?.let { revision ->
    validation?.let { revisionAfterValidation(revision, it) } ?: revision
  }

  fun save() {
    val normalizedId = packageId.trim()
    val normalizedName = displayName.trim()
    if (normalizedId.isEmpty() || normalizedName.isEmpty()) {
      localError = "Package ID and display name are required"
      return
    }
    localError = null
    onBusy("save")
    scope.launch {
      try {
        val savedDetail = withContext(Dispatchers.IO) {
          val target = if (creating) {
            runtimeClient.createSkillDraft(
              RuntimeSkillDraftRequest(
                packageId = normalizedId,
                displayName = normalizedName,
                description = description.trim(),
                kind = kind,
                requiredTools = splitTools(requiredTools),
              ),
            )
            runtimeClient.getSkillDetail(normalizedId)
          } else {
            checkNotNull(detail)
          }
          val revision = checkNotNull(target.editableDraft) { "Editable draft is unavailable" }
          runtimeClient.updateSkillDraft(
            revision.revisionId,
            draftUpdateFiles(target, revision, instructions, splitTools(requiredTools)),
          )
          runtimeClient.getSkillDetail(target.packageId)
        }
        onSaved(savedDetail)
      } catch (error: Throwable) {
        onFailure(error, "Unable to save draft")
      }
    }
  }

  fun validate() {
    val revision = detail?.editableDraft
    if (revision == null) {
      localError = "Save the draft before validation"
      return
    }
    localError = null
    onBusy("validate")
    scope.launch {
      try {
        val result = withContext(Dispatchers.IO) {
          runtimeClient.validateSkillDraft(revision.revisionId)
        }
        onValidated(result)
      } catch (error: Throwable) {
        onFailure(error, "Validation failed")
      }
    }
  }

  SkillDraftScreen(
    creating = creating,
    packageId = packageId,
    displayName = displayName,
    description = description,
    kind = kind,
    instructions = instructions,
    requiredTools = requiredTools,
    actions = actions,
    busyOperation = busyOperation,
    error = localError ?: externalError,
    validation = validation,
    canActivate = activationRevision?.validation?.ok == true,
    onPackageIdChange = { packageId = it },
    onDisplayNameChange = { displayName = it },
    onDescriptionChange = { description = it },
    onKindChange = { kind = it },
    onInstructionsChange = { instructions = it },
    onRequiredToolsChange = { requiredTools = it },
    onSave = ::save,
    onValidate = ::validate,
    onActivate = { activationRevision?.let(onActivate) },
    onBack = onBack,
    busy = busy,
  )
}

@Composable
fun SkillDraftScreen(
  creating: Boolean,
  packageId: String,
  displayName: String,
  description: String,
  kind: String,
  instructions: String,
  requiredTools: String,
  actions: Set<SkillAction>,
  busyOperation: String?,
  error: String?,
  validation: RuntimeSkillValidation?,
  canActivate: Boolean,
  onPackageIdChange: (String) -> Unit,
  onDisplayNameChange: (String) -> Unit,
  onDescriptionChange: (String) -> Unit,
  onKindChange: (String) -> Unit,
  onInstructionsChange: (String) -> Unit,
  onRequiredToolsChange: (String) -> Unit,
  onSave: () -> Unit,
  onValidate: () -> Unit,
  onActivate: () -> Unit,
  onBack: () -> Unit,
  busy: Boolean,
) {
  var kindMenuOpen by remember { mutableStateOf(false) }
  val fieldColors = OutlinedTextFieldDefaults.colors(
    focusedContainerColor = GaSurface,
    unfocusedContainerColor = GaSurface,
    disabledContainerColor = GaSurfaceSubtle,
    errorContainerColor = GaSurface,
  )
  Column(modifier = Modifier.fillMaxSize().background(GaSurface).imePadding()) {
    DraftTopBar(onBack)
    error?.let { InlineSkillError(it) }
    Column(
      modifier = Modifier
        .weight(1f)
        .fillMaxWidth()
        .background(GaSurface)
        .verticalScroll(rememberScrollState())
        .padding(horizontal = 16.dp, vertical = 16.dp),
      verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
      OutlinedTextField(
        value = packageId,
        onValueChange = onPackageIdChange,
        enabled = creating && !busy,
        label = { Text("Package ID") },
        singleLine = true,
        colors = fieldColors,
        modifier = Modifier.fillMaxWidth(),
      )
      OutlinedTextField(
        value = displayName,
        onValueChange = onDisplayNameChange,
        enabled = creating && !busy,
        label = { Text("Display name") },
        singleLine = true,
        colors = fieldColors,
        modifier = Modifier.fillMaxWidth(),
      )
      if (creating) {
        OutlinedTextField(
          value = description,
          onValueChange = onDescriptionChange,
          enabled = !busy,
          label = { Text("Description") },
          minLines = 2,
          maxLines = 3,
          colors = fieldColors,
          modifier = Modifier.fillMaxWidth(),
        )
      }
      Column {
        OutlinedButton(
          onClick = { kindMenuOpen = true },
          enabled = creating && !busy,
          modifier = Modifier.fillMaxWidth().height(56.dp),
        ) {
          Text("Kind: ${statusLabel(kind)}", modifier = Modifier.weight(1f))
          Icon(Icons.Outlined.ExpandMore, contentDescription = "Choose package kind")
        }
        DropdownMenu(expanded = kindMenuOpen, onDismissRequest = { kindMenuOpen = false }) {
          listOf("instruction_only", "host_tools_only").forEach { option ->
            DropdownMenuItem(
              text = { Text(statusLabel(option)) },
              onClick = {
                onKindChange(option)
                kindMenuOpen = false
              },
            )
          }
        }
      }
      OutlinedTextField(
        value = requiredTools,
        onValueChange = onRequiredToolsChange,
        enabled = !busy && (creating || SkillAction.Edit in actions),
        label = { Text("Required host tools") },
        supportingText = { Text("Comma-separated tool IDs") },
        minLines = 1,
        maxLines = 3,
        colors = fieldColors,
        modifier = Modifier.fillMaxWidth(),
      )
      OutlinedTextField(
        value = instructions,
        onValueChange = onInstructionsChange,
        enabled = !busy && (creating || SkillAction.Edit in actions),
        label = { Text("Instructions") },
        minLines = 7,
        colors = fieldColors,
        modifier = Modifier.fillMaxWidth(),
      )
      validation?.let { DraftValidationResult(it) }
      Spacer(Modifier.height(8.dp))
    }
    DraftActionBar(
      actions = actions,
      busyOperation = busyOperation,
      busy = busy,
      canActivate = canActivate,
      onSave = onSave,
      onValidate = onValidate,
      onActivate = onActivate,
    )
  }
}

@Composable
private fun DraftTopBar(onBack: () -> Unit) {
  Row(
    modifier = Modifier.fillMaxWidth().height(64.dp).padding(horizontal = 8.dp),
    verticalAlignment = Alignment.CenterVertically,
  ) {
    IconButton(onClick = onBack, modifier = Modifier.size(48.dp)) {
      Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = "Back from draft")
    }
    Text("Skill draft", style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
    Spacer(Modifier.size(48.dp))
  }
  HorizontalDivider(color = GaBorder)
}

@Composable
private fun DraftValidationResult(validation: RuntimeSkillValidation) {
  val container = if (validation.ok) GaReadyContainer else MaterialTheme.colorScheme.errorContainer
  val content = if (validation.ok) GaReady else MaterialTheme.colorScheme.error
  Column(
    modifier = Modifier.fillMaxWidth().background(container, GaLargeShape).padding(14.dp),
    verticalArrangement = Arrangement.spacedBy(6.dp),
  ) {
    Row(verticalAlignment = Alignment.CenterVertically) {
      Icon(Icons.Outlined.CheckCircle, contentDescription = null, tint = content, modifier = Modifier.size(20.dp))
      Text(
        if (validation.ok) "Validation passed" else "Validation failed",
        color = content,
        fontWeight = FontWeight.SemiBold,
        modifier = Modifier.padding(start = 8.dp),
      )
    }
    validation.errors.forEach { Text(it, color = content, fontSize = 13.sp, lineHeight = 19.sp) }
    validation.warnings.forEach { Text("Warning: $it", color = GaAmberText, fontSize = 13.sp, lineHeight = 19.sp) }
    Text(
      "Revision ${shortRevision(validation.revisionId)} • generation ${validation.snapshotGeneration}",
      color = GaTextSecondary,
      fontSize = 12.sp,
      lineHeight = 18.sp,
    )
  }
}

@Composable
private fun DraftActionBar(
  actions: Set<SkillAction>,
  busyOperation: String?,
  busy: Boolean,
  canActivate: Boolean,
  onSave: () -> Unit,
  onValidate: () -> Unit,
  onActivate: () -> Unit,
) {
  Column(modifier = Modifier.fillMaxWidth().background(GaSurface)) {
    HorizontalDivider(color = GaBorder)
    Row(
      modifier = Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 8.dp, vertical = 8.dp),
      horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
      if (SkillAction.Edit in actions || SkillAction.Create in actions) {
        FilledTonalButton(
          onClick = onSave,
          enabled = !busy,
          colors = ButtonDefaults.filledTonalButtonColors(
            containerColor = androidx.compose.ui.graphics.Color(0xFFD9F3EF),
            contentColor = GaPrimaryActive,
          ),
          modifier = Modifier.weight(1f),
        ) {
          DraftActionIcon(busyOperation == "save", Icons.Outlined.Save)
          Text("Save", maxLines = 1, overflow = TextOverflow.Clip, modifier = Modifier.padding(start = 4.dp))
        }
      }
      if (SkillAction.Validate in actions) {
        OutlinedButton(onClick = onValidate, enabled = !busy, modifier = Modifier.weight(1f)) {
          DraftActionIcon(busyOperation == "validate", Icons.Outlined.CheckCircle)
          Text("Validate", maxLines = 1, overflow = TextOverflow.Clip, modifier = Modifier.padding(start = 4.dp))
        }
      }
      if (SkillAction.Activate in actions) {
        Button(onClick = onActivate, enabled = !busy && canActivate, modifier = Modifier.weight(1f)) {
          DraftActionIcon(busyOperation == "activation", Icons.Outlined.Publish)
          Text("Activate", maxLines = 1, overflow = TextOverflow.Clip, modifier = Modifier.padding(start = 4.dp))
        }
      }
    }
  }
}

@Composable
private fun DraftActionIcon(
  busy: Boolean,
  icon: androidx.compose.ui.graphics.vector.ImageVector,
) {
  if (busy) {
    CircularProgressIndicator(modifier = Modifier.size(17.dp), strokeWidth = 2.dp)
  } else {
    Icon(icon, contentDescription = null, modifier = Modifier.size(17.dp))
  }
}

private fun splitTools(value: String): List<String> =
  value.split(',').map(String::trim).filter(String::isNotEmpty).distinct()
