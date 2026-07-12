package com.generalagent.mobile.ui

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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.DeleteOutline
import androidx.compose.material.icons.outlined.Edit
import androidx.compose.material.icons.outlined.PowerSettingsNew
import androidx.compose.material.icons.outlined.Publish
import androidx.compose.material.icons.outlined.Restore
import androidx.compose.material.icons.outlined.Visibility
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.generalagent.mobile.runtime.RuntimeSkillDetail
import com.generalagent.mobile.runtime.RuntimeSkillRevision

@Composable
fun SkillDetailScreen(
  detail: RuntimeSkillDetail,
  actions: Set<SkillAction>,
  busyOperation: String?,
  inlineError: String?,
  onRetry: () -> Unit,
  onBack: () -> Unit,
  onEdit: () -> Unit,
  onActivate: (RuntimeSkillRevision) -> Unit,
  onDisable: () -> Unit,
  onRollback: (RuntimeSkillRevision) -> Unit,
  onRemove: (RuntimeSkillRevision) -> Unit,
) {
  val draft = detail.editableDraft
  val active = detail.activeRevisionId?.let { activeId ->
    detail.revisions.find { it.revisionId == activeId && !it.editable && it.status == "managed" }
  }
  var selectedRollbackId by remember(detail.packageId, detail.activeRevisionId) {
    mutableStateOf<String?>(null)
  }
  val rollback = detail.revisions.firstOrNull { revision ->
    revision.revisionId == selectedRollbackId &&
      revision.validation.ok &&
      selectRollbackTarget(detail, revision) != null
  }
  val anyBusy = busyOperation != null
  val canOpenDraft = canOpenExistingDraft(actions)

  Column(modifier = Modifier.fillMaxSize().background(GaSurface)) {
    DetailTopBar(detail.displayName, onBack)
    inlineError?.let { InlineSkillError(it, onRetry) }
    Column(
      modifier = Modifier
        .weight(1f)
        .fillMaxWidth()
        .background(GaSurface)
        .verticalScroll(rememberScrollState())
        .padding(bottom = 20.dp),
    ) {
      DetailIdentity(detail)
      HorizontalDivider(color = GaBorder)
      SectionTitle("REVISIONS")
      detail.revisions.forEach { revision ->
        RevisionRow(
          revision = revision,
          active = revision.revisionId == detail.activeRevisionId,
          rollbackTarget = revision.revisionId == selectedRollbackId,
          onClick = when {
            revision.editable && canOpenDraft -> onEdit
            SkillAction.Rollback in actions && revision.validation.ok &&
              selectRollbackTarget(detail, revision) != null -> {
              { selectedRollbackId = revision.revisionId }
            }
            else -> null
          },
          draftCanEdit = SkillAction.Edit in actions,
        )
        HorizontalDivider(color = GaBorder, modifier = Modifier.padding(horizontal = 16.dp))
      }
      if (detail.revisions.isEmpty()) {
        Text(
          "No revision history is available.",
          color = GaTextSecondary,
          style = MaterialTheme.typography.bodyMedium,
          modifier = Modifier.padding(horizontal = 16.dp, vertical = 20.dp),
        )
      }
    }
    if (detail.sourceLayer == "managed" && actions.isNotEmpty()) {
      DetailActionBar(
        actions = actions,
        draft = draft,
        active = active,
        rollback = rollback,
        busy = anyBusy,
        busyOperation = busyOperation,
        onEdit = onEdit,
        onActivate = onActivate,
        onDisable = onDisable,
        onRollback = onRollback,
        onRemove = onRemove,
      )
    }
  }
}

@Composable
private fun DetailTopBar(title: String, onBack: () -> Unit) {
  Row(
    modifier = Modifier.fillMaxWidth().height(64.dp).padding(horizontal = 8.dp),
    verticalAlignment = Alignment.CenterVertically,
  ) {
    IconButton(onClick = onBack, modifier = Modifier.size(48.dp)) {
      Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = "Back to skills")
    }
    Text(
      title,
      style = MaterialTheme.typography.titleMedium,
      maxLines = 1,
      overflow = TextOverflow.Ellipsis,
      modifier = Modifier.weight(1f).padding(end = 48.dp),
    )
  }
  HorizontalDivider(color = GaBorder)
}

@Composable
private fun DetailIdentity(detail: RuntimeSkillDetail) {
  Column(
    modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 18.dp),
    verticalArrangement = Arrangement.spacedBy(8.dp),
  ) {
    Row(verticalAlignment = Alignment.CenterVertically) {
      Text(
        detail.displayName,
        style = MaterialTheme.typography.headlineSmall,
        modifier = Modifier.weight(1f),
      )
      SourceLabel(detail.sourceLayer)
    }
    Text(detail.packageId, color = GaTextSecondary, fontSize = 12.sp, lineHeight = 18.sp)
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
      DetailFact(
        "STATUS",
        statusLabel(detail.status),
        detail.status == "active",
        Modifier.weight(1f),
      )
      DetailFact("VERSION", detail.version.ifBlank { "Draft" }, true, Modifier.weight(1f))
    }
    Text(
      "Active revision: ${detail.activeRevisionId?.let(::shortRevision) ?: "None"}",
      style = MaterialTheme.typography.bodyMedium,
      color = GaTextSecondary,
    )
    if (detail.reason.isNotBlank()) {
      Text(detail.reason, color = GaAmberText, style = MaterialTheme.typography.bodyMedium)
    }
  }
}

@Composable
private fun DetailFact(label: String, value: String, positive: Boolean, modifier: Modifier) {
  Column(modifier = modifier.background(GaSurfaceSubtle, GaSmallShape).padding(10.dp)) {
    Text(label, style = MaterialTheme.typography.labelMedium, color = GaTextSecondary)
    Text(
      value,
      style = MaterialTheme.typography.bodyMedium,
      fontWeight = FontWeight.SemiBold,
      color = if (positive) GaText else GaAmberText,
      maxLines = 2,
    )
  }
}

@Composable
private fun SectionTitle(text: String) {
  Text(
    text,
    style = MaterialTheme.typography.labelMedium,
    color = GaTextSecondary,
    modifier = Modifier.padding(start = 16.dp, top = 20.dp, bottom = 8.dp),
  )
}

@Composable
private fun RevisionRow(
  revision: RuntimeSkillRevision,
  active: Boolean,
  rollbackTarget: Boolean,
  onClick: (() -> Unit)?,
  draftCanEdit: Boolean,
) {
  val modifier = Modifier
    .fillMaxWidth()
    .heightIn(min = 76.dp)
    .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
    .padding(horizontal = 16.dp, vertical = 11.dp)
  Row(modifier = modifier, verticalAlignment = Alignment.Top) {
    Box(
      modifier = Modifier.size(36.dp).background(
        if (revision.validation.ok) GaReadyContainer else GaAmberContainer,
        GaSmallShape,
      ),
      contentAlignment = Alignment.Center,
    ) {
      Icon(
        Icons.Outlined.CheckCircle,
        contentDescription = null,
        tint = if (revision.validation.ok) GaReady else GaAmber,
        modifier = Modifier.size(19.dp),
      )
    }
    Column(modifier = Modifier.weight(1f).padding(start = 12.dp)) {
      Row(verticalAlignment = Alignment.CenterVertically) {
        Text(
          "Version ${revision.version}",
          style = MaterialTheme.typography.bodyMedium,
          fontWeight = FontWeight.SemiBold,
          modifier = Modifier.weight(1f),
        )
        if (active) {
          Text("ACTIVE", color = GaReady, style = MaterialTheme.typography.labelMedium)
        } else if (rollbackTarget) {
          Text("ROLLBACK TARGET", color = GaPrimaryActive, style = MaterialTheme.typography.labelMedium)
        }
      }
      Text(
        "${shortRevision(revision.revisionId)} • ${statusLabel(revision.status)}",
        color = GaTextSecondary,
        fontSize = 12.sp,
        lineHeight = 18.sp,
      )
      Text(
        if (revision.validation.ok) "Validation passed" else "Validation required",
        color = if (revision.validation.ok) GaReady else GaAmberText,
        fontSize = 12.sp,
        lineHeight = 18.sp,
      )
    }
    if (revision.editable && onClick != null) {
      Icon(
        if (draftCanEdit) Icons.Outlined.Edit else Icons.Outlined.Visibility,
        contentDescription = if (draftCanEdit) "Edit draft" else "Open draft",
        modifier = Modifier.size(22.dp),
      )
    }
  }
}

@Composable
private fun DetailActionBar(
  actions: Set<SkillAction>,
  draft: RuntimeSkillRevision?,
  active: RuntimeSkillRevision?,
  rollback: RuntimeSkillRevision?,
  busy: Boolean,
  busyOperation: String?,
  onEdit: () -> Unit,
  onActivate: (RuntimeSkillRevision) -> Unit,
  onDisable: () -> Unit,
  onRollback: (RuntimeSkillRevision) -> Unit,
  onRemove: (RuntimeSkillRevision) -> Unit,
) {
  Column(modifier = Modifier.fillMaxWidth().background(GaSurface)) {
    HorizontalDivider(color = GaBorder)
    Row(
      modifier = Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 8.dp, vertical = 8.dp),
      horizontalArrangement = Arrangement.spacedBy(4.dp),
      verticalAlignment = Alignment.CenterVertically,
    ) {
      if (draft != null && canOpenExistingDraft(actions)) {
        TextButton(onClick = onEdit, enabled = !busy, modifier = Modifier.weight(1f)) {
          Icon(
            if (SkillAction.Edit in actions) Icons.Outlined.Edit else Icons.Outlined.Visibility,
            contentDescription = null,
            modifier = Modifier.size(18.dp),
          )
          Text(
            if (SkillAction.Edit in actions) "Edit" else "Open",
            modifier = Modifier.padding(start = 4.dp),
          )
        }
      }
      if (draft != null && draft.validation.ok && SkillAction.Activate in actions) {
        Button(onClick = { onActivate(draft) }, enabled = !busy, modifier = Modifier.weight(1f)) {
          ActionIcon(busyOperation == "activation", Icons.Outlined.Publish)
          Text("Activate", modifier = Modifier.padding(start = 4.dp))
        }
      }
      if (rollback != null && SkillAction.Rollback in actions) {
        FilledTonalButton(
          onClick = { onRollback(rollback) },
          enabled = !busy,
          colors = ButtonDefaults.filledTonalButtonColors(
            containerColor = Color(0xFFD9F3EF),
            contentColor = GaPrimaryActive,
          ),
          modifier = Modifier.weight(1f),
        ) {
          ActionIcon(busyOperation == "rollback", Icons.Outlined.Restore)
          Text("Rollback", modifier = Modifier.padding(start = 4.dp))
        }
      }
      if (SkillAction.Disable in actions) {
        IconButton(onClick = onDisable, enabled = !busy, modifier = Modifier.size(48.dp)) {
          ActionIcon(busyOperation == "disable", Icons.Outlined.PowerSettingsNew, "Disable skill")
        }
      }
      if (active != null && SkillAction.Delete in actions) {
        IconButton(onClick = { onRemove(active) }, enabled = !busy, modifier = Modifier.size(48.dp)) {
          ActionIcon(false, Icons.Outlined.DeleteOutline, "Remove skill", MaterialTheme.colorScheme.error)
        }
      }
    }
  }
}

@Composable
private fun ActionIcon(
  busy: Boolean,
  icon: androidx.compose.ui.graphics.vector.ImageVector,
  description: String? = null,
  tint: Color = Color.Unspecified,
) {
  if (busy) {
    CircularProgressIndicator(modifier = Modifier.size(18.dp), strokeWidth = 2.dp)
  } else {
    Icon(icon, contentDescription = description, tint = tint, modifier = Modifier.size(18.dp))
  }
}
