package com.generalagent.mobile.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.Code
import androidx.compose.material.icons.outlined.DesktopWindows
import androidx.compose.material.icons.outlined.Folder
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.outlined.Language
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material.icons.outlined.Terminal
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

@Composable
fun SkillsScreen(rows: List<SkillRow>, onBack: () -> Unit) {
  BackHandler(onBack = onBack)
  Column(modifier = Modifier.fillMaxSize().background(Color.White)) {
    SkillsTopBar(rows, onBack)
    Column(
      modifier = Modifier
        .weight(1f)
        .fillMaxWidth()
        .verticalScroll(rememberScrollState()),
    ) {
      if (rows.isEmpty()) {
        EmptySkillsState()
      } else {
        SkillSectionLabel("AVAILABLE")
        rows.filter { it.available }.forEachIndexed { index, row ->
          SkillListRow(row)
          if (index < rows.count { it.available } - 1) {
            HorizontalDivider(color = GaBorder, modifier = Modifier.padding(horizontal = 16.dp))
          }
        }
        HorizontalDivider(color = GaBorder)
        SkillSectionLabel("UNAVAILABLE")
        rows.filterNot { it.available }.forEachIndexed { index, row ->
          SkillListRow(row)
          if (index < rows.count { !it.available } - 1) {
            HorizontalDivider(color = GaBorder, modifier = Modifier.padding(horizontal = 16.dp))
          }
        }
      }
      Spacer(modifier = Modifier.height(16.dp))
    }
  }
}

@Composable
private fun SkillsTopBar(rows: List<SkillRow>, onBack: () -> Unit) {
  Row(
    modifier = Modifier.fillMaxWidth().height(72.dp).background(Color.White).padding(horizontal = 8.dp),
    verticalAlignment = Alignment.CenterVertically,
  ) {
    IconButton(onClick = onBack, modifier = Modifier.size(48.dp)) {
      Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = "Back", tint = GaTextSecondary)
    }
    Column(
      modifier = Modifier.weight(1f),
      horizontalAlignment = Alignment.CenterHorizontally,
      verticalArrangement = Arrangement.Center,
    ) {
      Text("Skills", style = MaterialTheme.typography.titleMedium)
      Text(
        "${rows.count { it.available }} available, ${rows.count { !it.available }} unavailable",
        color = GaTextSecondary,
        fontSize = 12.sp,
        lineHeight = 18.sp,
      )
    }
    IconButton(onClick = {}, modifier = Modifier.size(48.dp)) {
      Icon(Icons.Outlined.MoreVert, contentDescription = "More options", tint = GaTextSecondary)
    }
  }
  HorizontalDivider(color = GaBorder)
}

@Composable
private fun EmptySkillsState() {
  Column(
    modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 32.dp),
    horizontalAlignment = Alignment.CenterHorizontally,
    verticalArrangement = Arrangement.spacedBy(8.dp),
  ) {
    Icon(Icons.Outlined.Info, contentDescription = null, tint = GaTextSecondary)
    Text("No installed skills", color = GaText, fontWeight = FontWeight.Medium)
    Text(
      "The Android runtime did not discover any skills in app data.",
      color = GaTextSecondary,
      fontSize = 13.sp,
      lineHeight = 20.sp,
    )
  }
}

@Composable
private fun SkillSectionLabel(label: String) {
  Text(
    text = label,
    color = Color(0xFF4B5563),
    fontSize = 12.sp,
    lineHeight = 20.sp,
    fontWeight = FontWeight.SemiBold,
    letterSpacing = 0.sp,
    modifier = Modifier.padding(start = 16.dp, top = 24.dp, bottom = 8.dp),
  )
}

@Composable
private fun SkillListRow(row: SkillRow) {
  val accent = if (row.available) Color(0xFF16A34A) else GaAmber
  val container = if (row.available) Color(0xFFF0FDF4) else GaAmberContainer
  Row(
    modifier = Modifier.fillMaxWidth().padding(
      horizontal = 16.dp,
      vertical = if (row.available) 12.dp else 8.dp,
    ),
    verticalAlignment = Alignment.Top,
  ) {
    Box(
      modifier = Modifier.size(48.dp).background(container, GaSmallShape),
      contentAlignment = Alignment.Center,
    ) {
      Icon(row.skillIcon(), contentDescription = null, tint = accent, modifier = Modifier.size(22.dp))
    }
    Column(
      modifier = Modifier.weight(1f).padding(start = 12.dp, top = 3.dp, end = 8.dp),
      verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
      Text(
        text = row.name,
        color = GaText,
        fontSize = 14.sp,
        lineHeight = 22.sp,
        fontWeight = FontWeight.Medium,
      )
      Text(
        text = row.detail,
        color = if (row.available) GaTextSecondary else GaAmberText,
        fontSize = 12.sp,
        lineHeight = 20.sp,
      )
    }
    IconButton(onClick = {}, modifier = Modifier.size(48.dp)) {
      Icon(
        imageVector = if (row.available) Icons.Outlined.CheckCircle else Icons.Outlined.Info,
        contentDescription = if (row.available) "Ready" else "Unavailable details",
        tint = accent,
      )
    }
  }
}

private fun SkillRow.skillIcon(): ImageVector =
  when (name) {
    "Echo" -> Icons.Outlined.Terminal
    "Filesystem notes" -> Icons.Outlined.Folder
    "Web browser" -> Icons.Outlined.Language
    "Shell tools" -> Icons.Outlined.Code
    else -> Icons.Outlined.DesktopWindows
  }
