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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.generalagent.mobile.runtime.RuntimeDiagnostics
import com.generalagent.mobile.runtime.RuntimeSkill

@Composable
internal fun SkillInventoryRoute(
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
    state.inlineError?.let { InlineSkillError(it, onRetry = onRefresh) }
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
    IconButton(onClick = onRefresh, enabled = !busy, modifier = Modifier.size(48.dp)) {
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
        skill.activeRevisionId?.let { "Revision ${shortRevision(it)}" } ?: "No active revision",
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
