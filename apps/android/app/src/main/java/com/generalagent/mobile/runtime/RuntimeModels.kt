package com.generalagent.mobile.runtime

data class RuntimeInitRequest(
  val appDataDir: String,
  val cacheDir: String,
  val databasePath: String,
  val builtinSkillsDir: String,
  val managedSkillsDir: String,
  val stagingSkillsDir: String,
  val quarantineSkillsDir: String,
  val skillPolicy: RuntimeSkillPolicy,
  val actorContext: RuntimeActorContext,
  val platform: String = "android",
  val capabilities: List<String> = androidMvpCapabilities(),
)

data class RuntimeSkillPolicy(
  val mode: String = "disabled",
  val agentAuthoring: Boolean = false,
  val allowedKinds: List<String> = emptyList(),
  val protectedPackages: List<String> = emptyList(),
  val allowedOverrides: List<String> = emptyList(),
  val activationApprovalRequired: Boolean = true,
  val permissionEscalationApprovalRequired: Boolean = true,
  val rollbackApprovalRequired: Boolean = false,
)

data class RuntimeActorContext(
  val actorId: String = "anonymous",
  val role: String = "user",
  val tenantId: String? = null,
  val deviceId: String? = null,
  val grants: List<String> = emptyList(),
)

data class RuntimeDiagnostics(
  val platform: String,
  val capabilities: List<String>,
  val databaseReady: Boolean,
  val skillsReady: Boolean,
  val modelConfigured: Boolean,
  val skillManagementMode: String,
  val activeSnapshotGeneration: Long,
  val quarantinedCount: Int,
  val lastReloadStatus: String,
)

data class RuntimeSession(
  val id: String,
  val title: String,
  val createdAt: String,
  val updatedAt: String,
)

data class RuntimeMessage(
  val id: String,
  val sessionId: String,
  val role: String,
  val content: String,
  val createdAt: String,
)

data class RuntimeSkill(
  val packageId: String,
  val displayName: String,
  val version: String,
  val sourceLayer: String,
  val status: String,
  val available: Boolean,
  val reason: String,
  val activeRevisionId: String?,
  val manageable: Boolean,
  val description: String = "",
) {
  val id: String get() = packageId
  val label: String get() = displayName

  constructor(
    id: String,
    label: String,
    description: String,
    available: Boolean,
    reason: String,
  ) : this(
    packageId = id,
    displayName = label,
    version = "",
    sourceLayer = "builtin",
    status = if (available) "active" else "unavailable",
    available = available,
    reason = reason,
    activeRevisionId = null,
    manageable = false,
    description = description,
  )
}

data class RuntimeSkillPackageSummary(
  val packageId: String,
  val displayName: String,
  val version: String,
  val sourceLayer: String,
  val status: String,
  val reason: String,
  val activeRevisionId: String?,
)

data class RuntimeSkillRequirements(
  val runtimeTools: List<String>,
  val capabilities: List<String>,
  val connectors: List<String>,
  val packages: List<String>,
)

data class RuntimeSkillValidationSummary(
  val ok: Boolean,
  val errors: List<String>,
  val warnings: List<String>,
)

data class RuntimeSkillRevision(
  val revisionId: String,
  val version: String,
  val status: String,
  val editable: Boolean,
  val createdBy: String,
  val createdAt: String,
  val kind: String,
  val instructions: String,
  val validation: RuntimeSkillValidationSummary,
  val requirements: RuntimeSkillRequirements,
  val permissionDiffJson: String,
)

data class RuntimeSkillDetail(
  val packageId: String,
  val displayName: String,
  val version: String,
  val sourceLayer: String,
  val status: String,
  val reason: String,
  val activeRevisionId: String?,
  val revisions: List<RuntimeSkillRevision>,
  val editableDraft: RuntimeSkillRevision?,
)

data class RuntimeSkillDraftRequest(
  val packageId: String,
  val displayName: String,
  val description: String,
  val kind: String,
  val requiredTools: List<String>,
  val initialFiles: List<RuntimeSkillDraftFile> = emptyList(),
)

data class RuntimeSkillDraftFile(
  val path: String,
  val content: String,
)

data class RuntimeSkillDraftSummary(
  val packageId: String,
  val revisionId: String,
  val version: String,
  val kind: String,
  val status: String,
)

data class RuntimeSkillValidation(
  val ok: Boolean,
  val errors: List<String>,
  val warnings: List<String>,
  val requiredTools: List<String>,
  val requiredConnectors: List<String>,
  val dependencies: List<String>,
  val requiredCapabilities: List<String>,
  val resolverStatus: String,
  val resolverErrors: List<String>,
  val permissionDiffJson: String,
  val revisionId: String,
  val contentHash: String,
  val snapshotGeneration: Long,
)

data class RuntimeSkillApproval(
  val approvalId: String,
  val packageId: String,
  val permissionDiffJson: String,
  val requestedBy: String,
  val revisionId: String,
  val status: String,
)

data class RuntimeSkillMutation(
  val approvalId: String? = null,
  val packageId: String? = null,
  val revisionId: String? = null,
  val status: String = "published",
  val activeGeneration: Long = 0,
) {
  val approvalRequired: Boolean get() = approvalId != null && status == "pending"
}

sealed interface RuntimeSkillRollbackOutcome {
  data class Published(val mutation: RuntimeSkillMutation) : RuntimeSkillRollbackOutcome
  data class ApprovalRequired(val approval: RuntimeSkillApproval) : RuntimeSkillRollbackOutcome
}

data class RuntimeModelConfig(
  val providerId: String,
  val providerName: String,
  val endpointType: String,
  val baseUrl: String,
  val modelName: String,
  val secretId: String?,
  val headers: Map<String, String> = emptyMap(),
)

data class RuntimeTurn(val assistantText: String)
