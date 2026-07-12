package com.generalagent.mobile.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.WarningAmber
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp

@Composable
internal fun SkillApprovalDialog(
  state: SkillApprovalUiState,
  busy: Boolean,
  onDismiss: () -> Unit,
  onConfirm: () -> Unit,
) {
  val revision = state.revision
  val capabilities = approvalCapabilities(
    state.approval.permissionDiffJson,
    revision.requirements.capabilities,
  )
  val title = when (state.operation) {
    SkillApprovalOperation.Activation -> "Approve activation"
    SkillApprovalOperation.Rollback -> "Approve rollback"
    SkillApprovalOperation.Removal -> "Approve removal"
  }
  val consequence = when (state.operation) {
    SkillApprovalOperation.Activation -> "Approval publishes this revision in a new immutable skill snapshot."
    SkillApprovalOperation.Rollback -> "Approval replaces the active revision with this validated revision."
    SkillApprovalOperation.Removal -> "Approval removes this managed package from the active inventory."
  }
  AlertDialog(
    onDismissRequest = { if (!busy) onDismiss() },
    icon = {
      Icon(
        Icons.Outlined.WarningAmber,
        contentDescription = null,
        tint = if (state.operation == SkillApprovalOperation.Removal) {
          MaterialTheme.colorScheme.error
        } else {
          GaAmber
        },
      )
    },
    title = { Text(title) },
    text = {
      Column(
        modifier = Modifier.fillMaxWidth().heightIn(max = 470.dp).verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(14.dp),
      ) {
        ApprovalFact("Package", state.detail.displayName)
        ApprovalFact("Kind", statusLabel(revision.kind))
        ApprovalFact("Revision", revision.revisionId)
        ApprovalFact(
          "Validation",
          if (revision.validation.ok) "Passed" else revision.validation.errors.joinToString("; ").ifBlank { "Required" },
        )
        ApprovalList("Required tools", revision.requirements.runtimeTools)
        ApprovalList("Capability changes", capabilities)
        Text(consequence, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Medium)
        Text(
          "Requested by ${state.approval.requestedBy}",
          style = MaterialTheme.typography.labelMedium,
          color = GaTextSecondary,
        )
      }
    },
    dismissButton = {
      TextButton(onClick = onDismiss, enabled = !busy) { Text("Cancel") }
    },
    confirmButton = {
      Button(
        onClick = onConfirm,
        enabled = !busy && revision.validation.ok,
      ) {
        if (busy) {
          CircularProgressIndicator(modifier = Modifier.size(18.dp), strokeWidth = 2.dp)
        } else {
          Text(
            when (state.operation) {
              SkillApprovalOperation.Activation -> "Approve activation"
              SkillApprovalOperation.Rollback -> "Approve rollback"
              SkillApprovalOperation.Removal -> "Approve removal"
            },
          )
        }
      }
    },
    containerColor = GaSurface,
    iconContentColor = GaAmber,
    titleContentColor = GaText,
    textContentColor = GaTextSecondary,
  )
}

@Composable
private fun ApprovalFact(label: String, value: String) {
  Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
    Text(label, style = MaterialTheme.typography.labelMedium, color = GaTextSecondary)
    Text(value, style = MaterialTheme.typography.bodyMedium)
  }
}

@Composable
private fun ApprovalList(label: String, values: List<String>) {
  Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
    Text(label, style = MaterialTheme.typography.labelMedium, color = GaTextSecondary)
    if (values.isEmpty()) {
      Text("None", style = MaterialTheme.typography.bodyMedium, color = GaTextSecondary)
    } else {
      values.forEach { value ->
        Row(verticalAlignment = Alignment.Top) {
          Text("•", modifier = Modifier.padding(end = 8.dp))
          Text(value, style = MaterialTheme.typography.bodyMedium)
        }
      }
    }
  }
}
