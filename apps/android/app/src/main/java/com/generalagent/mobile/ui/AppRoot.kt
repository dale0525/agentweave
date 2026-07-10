package com.generalagent.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Chat
import androidx.compose.material.icons.filled.Analytics
import androidx.compose.material.icons.filled.Extension
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.outlined.Analytics
import androidx.compose.material.icons.outlined.ChatBubbleOutline
import androidx.compose.material.icons.outlined.Extension
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.zIndex
import com.generalagent.mobile.runtime.RuntimeClient
import com.generalagent.mobile.runtime.RuntimeDiagnostics
import com.generalagent.mobile.runtime.RuntimeSkill
import com.generalagent.mobile.secrets.ModelSecretStore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

enum class AppTab(val label: String) {
  Chat("Chat"),
  Settings("Settings"),
  Skills("Skills"),
  Diagnostics("Diagnostics"),
}

data class NavigationSize(val width: Int, val height: Int)

fun AppTab.activeNavigationSize(): NavigationSize =
  when (this) {
    AppTab.Chat -> NavigationSize(64, 48)
    AppTab.Settings -> NavigationSize(72, 60)
    AppTab.Skills -> NavigationSize(64, 48)
    AppTab.Diagnostics -> NavigationSize(72, 64)
  }

fun admittedAppTab(current: AppTab, requested: AppTab, settingsSaving: Boolean): AppTab =
  if (settingsSaving) current else requested

data class SkillRow(
  val name: String,
  val detail: String,
  val available: Boolean,
)

fun androidSkillRows(skills: List<RuntimeSkill>): List<SkillRow> =
  skills.map { skill ->
    SkillRow(
      name = skill.label,
      detail = if (skill.available) "Status: Ready" else skill.reason,
      available = skill.available,
    )
  }

fun androidDiagnosticCapabilityIds(): List<String> =
  listOf(
    "network.http",
    "filesystem.app_data",
    "secure_storage",
    "model.http_provider",
  )

@Composable
fun AppRoot(
  runtimeClient: RuntimeClient,
  turnGate: RuntimeTurnGate,
  settingsGate: RuntimeSettingsGate,
  initialDiagnostics: RuntimeDiagnostics,
  secretStore: ModelSecretStore,
  modifier: Modifier = Modifier,
) {
  var selectedTab by remember { mutableStateOf(AppTab.Chat) }
  var diagnostics by remember(initialDiagnostics) { mutableStateOf(initialDiagnostics) }
  var skillRows by remember { mutableStateOf<List<SkillRow>>(emptyList()) }
  val chatSending by turnGate.inFlight.collectAsState()
  val settingsSaving by settingsGate.inFlight.collectAsState()
  val settingsCompletionVersion by settingsGate.completionVersion.collectAsState()
  val scope = rememberCoroutineScope()

  LaunchedEffect(runtimeClient) {
    try {
      skillRows = withContext(Dispatchers.IO) {
        androidSkillRows(runtimeClient.listSkills())
      }
    } catch (cancelled: CancellationException) {
      throw cancelled
    }
  }
  val refreshDiagnostics = {
    scope.launch {
      runCatching { withContext(Dispatchers.IO) { runtimeClient.diagnostics() } }
        .onSuccess { diagnostics = it }
    }
    Unit
  }

  LaunchedEffect(settingsCompletionVersion) {
    if (settingsCompletionVersion > 0) refreshDiagnostics()
  }

  Column(
    modifier = modifier
      .fillMaxSize()
      .background(GaSurface),
  ) {
    Box(modifier = Modifier.weight(1f).fillMaxWidth()) {
      ChatScreen(
        runtimeClient = runtimeClient,
        turnGate = turnGate,
        diagnostics = diagnostics,
        secretStore = secretStore,
        onRefreshDiagnostics = refreshDiagnostics,
        interactionAllowed = { selectedTab == AppTab.Chat && !settingsSaving },
        modifier = if (selectedTab == AppTab.Chat) {
          Modifier.fillMaxSize().zIndex(1f)
        } else {
          Modifier
            .fillMaxSize()
            .graphicsLayer(alpha = 0f)
            .clearAndSetSemantics { }
            .zIndex(0f)
        },
      )
      if (selectedTab != AppTab.Chat) {
        Box(modifier = Modifier.fillMaxSize().zIndex(1f)) {
          when (selectedTab) {
            AppTab.Chat -> Unit
            AppTab.Settings -> SettingsScreen(
              runtimeClient = runtimeClient,
              secretStore = secretStore,
              settingsGate = settingsGate,
              runtimeBusy = chatSending,
              onBack = {
                selectedTab = admittedAppTab(selectedTab, AppTab.Chat, settingsSaving)
              },
              onSaved = refreshDiagnostics,
            )
            AppTab.Skills -> SkillsScreen(
              rows = skillRows,
              onBack = { selectedTab = AppTab.Chat },
            )
            AppTab.Diagnostics -> DiagnosticsScreen(
              diagnostics = diagnostics,
              skillRows = skillRows,
              onRefresh = refreshDiagnostics,
            )
          }
        }
      }
    }
    AppBottomNavigation(
      selected = selectedTab,
      onSelect = {
        selectedTab = admittedAppTab(selectedTab, it, settingsSaving)
      },
    )
  }
}

@Composable
private fun AppBottomNavigation(
  selected: AppTab,
  onSelect: (AppTab) -> Unit,
) {
  Column(
    modifier = Modifier
      .fillMaxWidth()
      .height(80.dp)
      .background(Color.White),
  ) {
    HorizontalDivider(color = GaBorder)
    Row(
      modifier = Modifier.fillMaxSize(),
      horizontalArrangement = Arrangement.SpaceEvenly,
      verticalAlignment = Alignment.CenterVertically,
    ) {
      AppTab.entries.forEach { tab ->
        val active = tab == selected
        val activeSize = tab.activeNavigationSize()
        Box(
          modifier = Modifier
            .weight(1f)
            .fillMaxHeight()
            .clickable { onSelect(tab) },
          contentAlignment = Alignment.Center,
        ) {
          Column(
            modifier = Modifier
              .size(width = activeSize.width.dp, height = activeSize.height.dp)
              .background(
                color = if (active) GaPrimaryActive else Color.Transparent,
                shape = GaLargeShape,
              ),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
          ) {
            Icon(
              imageVector = tab.icon(active),
              contentDescription = tab.label,
              tint = if (active) Color.White else GaTextSecondary,
              modifier = Modifier.size(24.dp),
            )
            Text(
              text = tab.label,
              color = if (active) Color.White else GaTextSecondary,
              style = MaterialTheme.typography.labelSmall.copy(
                fontSize = 11.sp,
                lineHeight = 16.sp,
                fontWeight = FontWeight.Medium,
                letterSpacing = 0.sp,
              ),
            )
          }
        }
      }
    }
  }
}

private fun AppTab.icon(active: Boolean): ImageVector =
  when (this) {
    AppTab.Chat -> if (active) Icons.AutoMirrored.Filled.Chat else Icons.Outlined.ChatBubbleOutline
    AppTab.Settings -> if (active) Icons.Filled.Settings else Icons.Outlined.Settings
    AppTab.Skills -> if (active) Icons.Filled.Extension else Icons.Outlined.Extension
    AppTab.Diagnostics -> if (active) Icons.Filled.Analytics else Icons.Outlined.Analytics
  }
