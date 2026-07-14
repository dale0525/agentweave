package com.generalagent.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Android
import androidx.compose.material.icons.outlined.Api
import androidx.compose.material.icons.outlined.Cable
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.Folder
import androidx.compose.material.icons.outlined.Lock
import androidx.compose.material.icons.outlined.Public
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material.icons.outlined.Warning
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.generalagent.mobile.runtime.RuntimeDiagnostics

@Composable
fun DiagnosticsScreen(
  diagnostics: RuntimeDiagnostics,
  skillRows: List<SkillRow>,
  onRefresh: () -> Unit,
) {
  Column(modifier = Modifier.fillMaxSize().background(GaSurface)) {
    DiagnosticsTopBar(onRefresh)
    Column(
      modifier = Modifier
        .weight(1f)
        .fillMaxWidth()
        .verticalScroll(rememberScrollState()),
    ) {
      Column(
        modifier = Modifier
          .fillMaxWidth()
          .padding(start = 16.dp, top = 24.dp, end = 16.dp, bottom = 8.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
      ) {
        DiagnosticSectionLabel("SYSTEM STATUS", startPadding = 8)
        DiagnosticStatusRow("Runtime initialized", "READY", GaReady, GaReadyContainer, Icons.Outlined.CheckCircle)
        DiagnosticStatusRow(
          "SQLite database",
          if (diagnostics.databaseReady) "READY" else "UNAVAILABLE",
          if (diagnostics.databaseReady) GaReady else MaterialTheme.colorScheme.error,
          if (diagnostics.databaseReady) GaReadyContainer else MaterialTheme.colorScheme.errorContainer,
          Icons.Outlined.CheckCircle,
        )
        DiagnosticStatusRow(
          "HTTP model",
          if (diagnostics.modelConfigured) "CONFIGURED" else "NOT CONFIGURED",
          if (diagnostics.modelConfigured) GaConfigured else GaTextSecondary,
          if (diagnostics.modelConfigured) GaConfiguredContainer else GaSurfaceMuted,
          Icons.Outlined.Settings,
        )
        SkillValidationStatus(skillRows)
        DiagnosticStatusRow("Native bridge", "LOADED", GaReady, GaReadyContainer, Icons.Outlined.Cable)
      }
      HorizontalDivider(color = GaBorder)
      Column(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 24.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
      ) {
        DiagnosticSectionLabel("CAPABILITIES")
        androidDiagnosticCapabilityIds().forEach { capability ->
          CapabilityRow(capability)
        }
      }
    }
  }
}

@Composable
private fun DiagnosticsTopBar(onRefresh: () -> Unit) {
  Row(
    modifier = Modifier.fillMaxWidth().height(64.dp).background(GaSurface).padding(horizontal = 16.dp),
    verticalAlignment = Alignment.CenterVertically,
  ) {
    IconButton(onClick = {}, modifier = Modifier.size(48.dp)) {
      Icon(Icons.Outlined.Android, contentDescription = "Android", tint = GaTextSecondary)
    }
    Text(
      "Diagnostics",
      style = MaterialTheme.typography.headlineSmall,
      modifier = Modifier.weight(1f),
      textAlign = androidx.compose.ui.text.style.TextAlign.Center,
    )
    IconButton(onClick = onRefresh, modifier = Modifier.size(48.dp)) {
      Icon(Icons.Outlined.Refresh, contentDescription = "Refresh", tint = GaTextSecondary)
    }
  }
  HorizontalDivider(color = GaBorder)
}

@Composable
private fun DiagnosticSectionLabel(label: String, startPadding: Int = 0) {
  Text(
    text = label,
    color = GaTextSecondary,
    fontSize = 14.sp,
    lineHeight = 22.sp,
    fontWeight = FontWeight.Bold,
    letterSpacing = 0.sp,
    modifier = Modifier.padding(start = startPadding.dp, bottom = 8.dp),
  )
}

@Composable
private fun DiagnosticStatusRow(
  label: String,
  status: String,
  statusColor: Color,
  containerColor: Color,
  icon: ImageVector,
) {
  val minimumWidth = when (status) {
    "READY" -> 84.dp
    "LOADED" -> 95.dp
    "CONFIGURED" -> 112.dp
    else -> 120.dp
  }
  Row(
    modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(horizontal = 8.dp, vertical = 6.dp),
    verticalAlignment = Alignment.CenterVertically,
  ) {
    Text(label, color = GaText, fontSize = 14.sp, lineHeight = 22.sp, fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f))
    Row(
      modifier = Modifier
        .widthIn(min = minimumWidth)
        .height(28.dp)
        .background(containerColor, GaSmallShape)
        .padding(horizontal = 10.dp),
      verticalAlignment = Alignment.CenterVertically,
      horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
      Icon(icon, contentDescription = null, tint = statusColor, modifier = Modifier.size(16.dp))
      Text(status, color = statusColor, fontSize = 11.sp, lineHeight = 16.sp, fontWeight = FontWeight.Bold)
    }
  }
}

@Composable
private fun SkillValidationStatus(rows: List<SkillRow>) {
  val unavailable = rows.count { !it.available }
  val badge = when {
    rows.isEmpty() -> "NONE INSTALLED"
    unavailable == 0 -> "ALL AVAILABLE"
    else -> "$unavailable UNAVAILABLE BY\nCAPABILITY"
  }
  val description = when {
    rows.isEmpty() -> "No installed skills were discovered in app data."
    unavailable == 0 -> "All installed skills satisfy Android capability requirements."
    else -> rows.filterNot { it.available }.joinToString("; ") { it.detail }
  }
  Column(
    modifier = Modifier
      .fillMaxWidth()
      .heightIn(min = 112.dp)
      .background(GaAmberContainer, GaLargeShape)
      .border(1.dp, GaAmber, GaLargeShape)
      .padding(8.dp),
    verticalArrangement = Arrangement.spacedBy(8.dp),
  ) {
    Row(
      verticalAlignment = Alignment.CenterVertically,
      horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
      Text(
        "Skill validation",
        color = GaAmberText,
        fontSize = 14.sp,
        lineHeight = 20.sp,
        fontWeight = FontWeight.Bold,
        modifier = Modifier.width(90.dp),
      )
      Row(
        modifier = Modifier
          .weight(1f)
          .height(40.dp)
          .background(GaAmberContainer, GaSmallShape)
          .padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(4.dp),
      ) {
        Icon(Icons.Outlined.Warning, contentDescription = null, tint = GaAmber, modifier = Modifier.size(16.dp))
        Text(
          badge,
          color = GaAmber,
          fontSize = 10.sp,
          lineHeight = 13.sp,
          fontWeight = FontWeight.Bold,
        )
      }
    }
    Text(
      description,
      color = GaAmberText,
      fontSize = 14.sp,
      lineHeight = 20.sp,
    )
  }
}

@Composable
private fun CapabilityRow(capability: String) {
  Row(
    modifier = Modifier
      .fillMaxWidth()
      .height(48.dp)
      .background(GaSurface, GaLargeShape)
      .border(1.dp, GaBorder, GaLargeShape)
      .padding(horizontal = 16.dp),
    verticalAlignment = Alignment.CenterVertically,
  ) {
    Icon(
      capabilityIcon(capability),
      contentDescription = null,
      tint = GaTextSecondary,
      modifier = Modifier.size(20.dp),
    )
    Spacer(modifier = Modifier.size(10.dp))
    Text(
      capability,
      color = GaTextSecondary,
      fontSize = 12.sp,
      lineHeight = 18.sp,
      fontWeight = FontWeight.Medium,
    )
  }
}

private fun capabilityIcon(capability: String): ImageVector =
  when (capability) {
    "network.http" -> Icons.Outlined.Public
    "filesystem.app_data" -> Icons.Outlined.Folder
    "secure_storage" -> Icons.Outlined.Lock
    else -> Icons.Outlined.Api
  }
