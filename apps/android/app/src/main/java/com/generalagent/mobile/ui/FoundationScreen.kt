package com.generalagent.mobile.ui

import android.content.Intent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.DeleteOutline
import androidx.compose.material.icons.outlined.Download
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.generalagent.mobile.runtime.RuntimeClient
import com.generalagent.mobile.runtime.RuntimeMailAccountStatus
import com.generalagent.mobile.runtime.RuntimeMemory
import com.generalagent.mobile.runtime.RuntimePendingFoundationAction
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

enum class FoundationSection(val label: String) {
  Accounts("Accounts"),
  Memory("Memory"),
  Actions("Actions"),
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FoundationScreen(
  runtimeClient: RuntimeClient,
  appDisplayName: String,
  modifier: Modifier = Modifier,
) {
  var section by remember { mutableStateOf(FoundationSection.Accounts) }
  var accounts by remember { mutableStateOf<List<RuntimeMailAccountStatus>>(emptyList()) }
  var memories by remember { mutableStateOf<List<RuntimeMemory>>(emptyList()) }
  var actions by remember { mutableStateOf<List<RuntimePendingFoundationAction>>(emptyList()) }
  var selectedMemory by remember { mutableStateOf<RuntimeMemory?>(null) }
  var selectedAction by remember { mutableStateOf<RuntimePendingFoundationAction?>(null) }
  var forgetCandidate by remember { mutableStateOf<RuntimeMemory?>(null) }
  var disconnectCandidate by remember { mutableStateOf<RuntimeMailAccountStatus?>(null) }
  var loading by remember { mutableStateOf(false) }
  var error by remember { mutableStateOf<String?>(null) }
  val scope = rememberCoroutineScope()
  val context = LocalContext.current

  suspend fun refresh() {
    loading = true
    error = null
    try {
      when (section) {
        FoundationSection.Accounts -> accounts = withContext(Dispatchers.IO) {
          runtimeClient.listMailAccounts().map { account ->
            runtimeClient.mailAccountStatus(account.id)
          }
        }
        FoundationSection.Memory -> memories = withContext(Dispatchers.IO) {
          runtimeClient.listMemories()
        }
        FoundationSection.Actions -> actions = withContext(Dispatchers.IO) {
          runtimeClient.listFoundationActions()
        }
      }
    } catch (cancelled: CancellationException) {
      throw cancelled
    } catch (failure: Throwable) {
      error = failure.message ?: "Foundation request failed"
    } finally {
      loading = false
    }
  }

  fun changeAccountConnection(status: RuntimeMailAccountStatus, connect: Boolean) {
    scope.launch {
      runCatching {
        withContext(Dispatchers.IO) {
          if (connect) {
            runtimeClient.connectMailAccount(status.account.id)
          } else {
            runtimeClient.disconnectMailAccount(status.account.id)
          }
        }
      }.onSuccess { updated ->
        accounts = accounts.map { item ->
          if (item.account.id == updated.account.id) updated else item
        }
        disconnectCandidate = null
      }.onFailure { failure ->
        error = failure.message ?: "Account action failed"
      }
    }
  }

  LaunchedEffect(section, runtimeClient) { refresh() }

  Column(modifier = modifier.fillMaxSize().background(GaSurface)) {
    Row(
      modifier = Modifier.fillMaxWidth().padding(horizontal = 18.dp, vertical = 14.dp),
      verticalAlignment = Alignment.CenterVertically,
    ) {
      Column(modifier = Modifier.weight(1f)) {
        Text("TRUSTED DATA", style = MaterialTheme.typography.labelMedium, color = GaAmberText)
        Text(appDisplayName, style = MaterialTheme.typography.headlineSmall)
      }
      IconButton(onClick = { scope.launch { refresh() } }, enabled = !loading) {
        Icon(Icons.Outlined.Refresh, contentDescription = "Refresh trusted data")
      }
    }
    HorizontalDivider(color = GaBorder)
    Row(
      modifier = Modifier.fillMaxWidth().padding(12.dp),
      horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
      FoundationSection.entries.forEach { item ->
        val selected = section == item
        if (selected) {
          Button(
            onClick = { section = item },
            modifier = Modifier.weight(1f).heightIn(min = 48.dp),
          ) { Text(item.label) }
        } else {
          OutlinedButton(
            onClick = { section = item },
            modifier = Modifier.weight(1f).heightIn(min = 48.dp),
          ) { Text(item.label) }
        }
      }
    }
    if (error != null) {
      Text(
        text = checkNotNull(error),
        color = MaterialTheme.colorScheme.error,
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
      )
    }
    if (loading) {
      Text(
        "Loading scoped ${section.label.lowercase()}…",
        color = GaTextSecondary,
        modifier = Modifier.padding(18.dp),
      )
    } else {
      when (section) {
        FoundationSection.Accounts -> AccountLedger(
          accounts = accounts,
          onToggle = { status ->
            if (accountActionNeedsConfirmation(status.state)) {
              disconnectCandidate = status
            } else {
              changeAccountConnection(status, connect = true)
            }
          },
        )
        FoundationSection.Memory -> MemoryLedger(
          memories = memories,
          onOpen = { selectedMemory = it },
          onExport = {
            scope.launch {
              runCatching { withContext(Dispatchers.IO) { runtimeClient.exportMemories() } }
                .onSuccess { json ->
                  context.startActivity(
                    Intent.createChooser(
                      Intent(Intent.ACTION_SEND)
                        .setType("application/json")
                        .putExtra(Intent.EXTRA_TEXT, json),
                      "Export Memory ledger",
                    ),
                  )
                }
                .onFailure { failure -> error = failure.message ?: "Memory export failed" }
            }
          },
        )
        FoundationSection.Actions -> ActionLedger(
          actions = actions,
          onOpen = { selectedAction = it },
        )
      }
    }
  }

  selectedMemory?.let { memory ->
    ModalBottomSheet(onDismissRequest = { selectedMemory = null }) {
      MemoryDetail(
        memory = memory,
        onForget = {
          forgetCandidate = memory
          selectedMemory = null
        },
      )
    }
  }
  selectedAction?.let { item ->
    ModalBottomSheet(onDismissRequest = { selectedAction = null }) {
      ActionDetail(
        item = item,
        onResolve = { approve ->
          scope.launch {
            runCatching {
              withContext(Dispatchers.IO) {
                runtimeClient.resolveFoundationAction(item.approval.approvalId, approve)
              }
            }.onSuccess { resolved ->
              actions = actions.map { current ->
                if (current.approval.approvalId == resolved.approval.approvalId) {
                  current.copy(approval = resolved.approval, action = resolved.action)
                } else current
              }
              selectedAction = null
            }.onFailure { failure -> error = failure.message ?: "Action resolution failed" }
          }
        },
      )
    }
  }
  forgetCandidate?.let { memory ->
    AlertDialog(
      onDismissRequest = { forgetCandidate = null },
      title = { Text("Forget this memory?") },
      text = { Text("The value and evidence will be scrubbed from the scoped Memory provider.") },
      confirmButton = {
        Button(onClick = {
          scope.launch {
            runCatching {
              withContext(Dispatchers.IO) {
                runtimeClient.forgetMemory(memory.id, memory.version)
              }
            }.onSuccess {
              memories = memories.filterNot { it.id == memory.id }
              forgetCandidate = null
            }.onFailure { failure -> error = failure.message ?: "Forget failed" }
          }
        }) { Text("Forget permanently") }
      },
      dismissButton = {
        OutlinedButton(onClick = { forgetCandidate = null }) { Text("Keep memory") }
      },
    )
  }
  disconnectCandidate?.let { status ->
    AlertDialog(
      onDismissRequest = { disconnectCandidate = null },
      title = { Text("Disconnect mail account?") },
      text = {
        Text(
          "${status.account.displayName} (${status.account.primaryAddress.address}) will stop " +
            "mail reads and sends. Its secret remains protected in the host vault.",
        )
      },
      confirmButton = {
        Button(
          onClick = { changeAccountConnection(status, connect = false) },
        ) { Text("Disconnect") }
      },
      dismissButton = {
        OutlinedButton(onClick = { disconnectCandidate = null }) { Text("Keep connected") }
      },
    )
  }
}

fun accountActionNeedsConfirmation(state: String): Boolean = state == "connected"

@Composable
private fun AccountLedger(
  accounts: List<RuntimeMailAccountStatus>,
  onToggle: (RuntimeMailAccountStatus) -> Unit,
) {
  LedgerListEmpty(values = accounts, emptyTitle = "No Mail accounts") { status ->
    Card(
      colors = CardDefaults.cardColors(containerColor = GaSurfaceSubtle),
      modifier = Modifier.fillMaxWidth().border(1.dp, GaBorder, RoundedCornerShape(10.dp)),
    ) {
      Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
          Column(modifier = Modifier.weight(1f)) {
            Text(status.account.displayName, fontWeight = FontWeight.Bold)
            Text(status.account.primaryAddress.address, color = GaTextSecondary)
          }
          StatusPill(status.state.replace('_', ' '))
        }
        Text(status.detail ?: "Credentials remain in the host vault.", color = GaTextSecondary)
        OutlinedButton(
          onClick = { onToggle(status) },
          modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp),
        ) { Text(if (status.state == "connected") "Disconnect" else "Connect") }
      }
    }
  }
}

@Composable
private fun MemoryLedger(
  memories: List<RuntimeMemory>,
  onOpen: (RuntimeMemory) -> Unit,
  onExport: () -> Unit,
) {
  Column(modifier = Modifier.fillMaxSize()) {
    OutlinedButton(
      onClick = onExport,
      modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp).heightIn(min = 48.dp),
    ) {
      Icon(Icons.Outlined.Download, contentDescription = null)
      Text(" Export ledger")
    }
    LedgerListEmpty(values = memories, emptyTitle = "Nothing committed here") { memory ->
      Card(
        modifier = Modifier
          .fillMaxWidth()
          .clickable { onOpen(memory) }
          .border(1.dp, GaBorder, RoundedCornerShape(10.dp)),
      ) {
        Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(5.dp)) {
          Text(memory.kind.uppercase(), style = MaterialTheme.typography.labelMedium, color = GaAmberText)
          Text(memory.text, fontWeight = FontWeight.Bold, maxLines = 2, overflow = TextOverflow.Ellipsis)
          Text("v${memory.version} · ${memory.sensitivity}", color = GaTextSecondary)
        }
      }
    }
  }
}

@Composable
private fun ActionLedger(
  actions: List<RuntimePendingFoundationAction>,
  onOpen: (RuntimePendingFoundationAction) -> Unit,
) {
  LedgerListEmpty(values = actions, emptyTitle = "No actions awaiting review") { item ->
    Card(
      modifier = Modifier
        .fillMaxWidth()
        .clickable { onOpen(item) }
        .border(1.dp, GaBorder, RoundedCornerShape(10.dp)),
    ) {
      Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(5.dp)) {
        Text(
          if (item.approval.status == "pending") "AWAITING APPROVAL" else item.action.status.uppercase(),
          style = MaterialTheme.typography.labelMedium,
          color = GaAmberText,
        )
        Text(item.preview?.subject ?: item.approval.actionName, fontWeight = FontWeight.Bold)
        Text(item.approval.resourceTarget, color = GaTextSecondary)
      }
    }
  }
}

@Composable
private fun <T> LedgerListEmpty(
  values: List<T>,
  emptyTitle: String,
  content: @Composable (T) -> Unit,
) {
  if (values.isEmpty()) {
    Column(
      modifier = Modifier.fillMaxWidth().padding(28.dp),
      horizontalAlignment = Alignment.CenterHorizontally,
    ) {
      Text(emptyTitle, fontWeight = FontWeight.Bold)
      Text("Scoped host data will appear here.", color = GaTextSecondary)
    }
  } else {
    LazyColumn(
      modifier = Modifier.fillMaxSize(),
      contentPadding = androidx.compose.foundation.layout.PaddingValues(16.dp),
      verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
      items(values) { item -> content(item) }
    }
  }
}

@Composable
private fun MemoryDetail(memory: RuntimeMemory, onForget: () -> Unit) {
  Column(
    modifier = Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp),
    verticalArrangement = Arrangement.spacedBy(14.dp),
  ) {
    Text(memory.kind.uppercase(), style = MaterialTheme.typography.labelMedium, color = GaAmberText)
    Text(memory.text, style = MaterialTheme.typography.headlineSmall)
    StatusPill(memory.sensitivity)
    DetailRow("Confidence", "${memory.confidence / 100}%")
    DetailRow("Retention", memory.retention)
    DetailRow("Version", "v${memory.version}")
    Text("Evidence & provenance", fontWeight = FontWeight.Bold)
    memory.evidence.forEach { evidence ->
      Column(modifier = Modifier.fillMaxWidth().background(GaSurfaceMuted).padding(12.dp)) {
        Text(evidence.source.replace('_', ' '), fontWeight = FontWeight.Bold)
        evidence.excerpt?.let { Text("“$it”", color = GaTextSecondary) }
      }
    }
    OutlinedButton(
      onClick = onForget,
      modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp),
    ) {
      Icon(Icons.Outlined.DeleteOutline, contentDescription = null)
      Text(" Forget")
    }
    Spacer(Modifier.height(20.dp))
  }
}

@Composable
private fun ActionDetail(item: RuntimePendingFoundationAction, onResolve: (Boolean) -> Unit) {
  val preview = item.preview
  val pending = item.approval.status == "pending" && item.action.status == "waiting_approval"
  Column(
    modifier = Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp),
    verticalArrangement = Arrangement.spacedBy(14.dp),
  ) {
    Text("MAIL SEND", style = MaterialTheme.typography.labelMedium, color = GaAmberText)
    Text(preview?.subject ?: item.approval.actionName, style = MaterialTheme.typography.headlineSmall)
    Text(item.approval.riskSummary, color = GaAmberText)
    preview?.let {
      DetailRow("Account", it.accountId)
      DetailRow("From", formatAddress(it.from.name, it.from.address))
      DetailRow("To", it.to.joinToString { address -> formatAddress(address.name, address.address) })
      DetailRow("CC / BCC", (it.cc + it.bcc).joinToString { address -> address.address }.ifEmpty { "None" })
      DetailRow("Draft revision", "v${it.draftRevision}")
      DetailRow("Attachments", it.attachmentCount.toString())
      HashRow("Preview", it.previewHash)
    }
    HashRow("Arguments", item.approval.argumentsSha256)
    item.action.lastError?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    if (pending) {
      Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
      ) {
        OutlinedButton(
          onClick = { onResolve(false) },
          modifier = Modifier.weight(1f).heightIn(min = 48.dp),
        ) { Text("Reject") }
        Button(
          onClick = { onResolve(true) },
          modifier = Modifier.weight(1f).heightIn(min = 48.dp),
        ) { Text("Approve once") }
      }
    } else {
      Text("This action no longer accepts a decision.", color = GaTextSecondary)
    }
    Spacer(Modifier.height(20.dp))
  }
}

@Composable
private fun StatusPill(text: String) {
  Text(
    text = text,
    style = MaterialTheme.typography.labelMedium,
    color = GaReady,
    modifier = Modifier.background(GaReadyContainer, RoundedCornerShape(999.dp)).padding(horizontal = 10.dp, vertical = 5.dp),
  )
}

@Composable
private fun DetailRow(label: String, value: String) {
  Column {
    Text(label, style = MaterialTheme.typography.labelMedium, color = GaTextSecondary)
    Text(value, fontWeight = FontWeight.SemiBold)
  }
}

@Composable
private fun HashRow(label: String, value: String) {
  Column {
    Text(label, style = MaterialTheme.typography.labelMedium, color = GaTextSecondary)
    Text(value, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodyMedium)
  }
}

private fun formatAddress(name: String?, address: String): String =
  if (name.isNullOrBlank()) address else "$name <$address>"
