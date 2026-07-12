package com.generalagent.mobile.ui

import com.generalagent.mobile.runtime.RuntimeDiagnostics
import com.generalagent.mobile.runtime.RuntimeSkill
import com.generalagent.mobile.runtime.RuntimeSkillDetail
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SkillScreenStateTest {
  @Test
  fun productionParentSnapshotHelperPreservesDetail() {
    val detail = RuntimeSkillDetail(
      packageId = "com.example.owner",
      displayName = "Owner",
      version = "1.0.0",
      sourceLayer = "managed",
      status = "active",
      reason = "",
      activeRevisionId = "revision-active",
      revisions = emptyList(),
      editableDraft = null,
    )
    val state = SkillManagementUiState(listOf(skill("old")), diagnostics(7), detail = detail)

    val updated = parentSkillSnapshotUpdated(state, listOf(skill("new")), diagnostics(8), null)

    assertEquals(detail, updated.detail)
    assertEquals("new", updated.inventory.single().activeRevisionId)
    assertEquals(8L, updated.diagnostics.activeSnapshotGeneration)
  }

  @Test
  fun productionOperationGenerationRejectsLateResultsAfterNavigation() {
    val guard = SkillOperationGeneration()
    val detailLoad = guard.begin()
    assertTrue(guard.accepts(detailLoad))

    guard.invalidate()

    assertFalse(guard.accepts(detailLoad))
    assertTrue(guard.accepts(guard.begin()))
  }

  private fun skill(revision: String) = RuntimeSkill(
    packageId = "com.example.owner",
    displayName = "Owner",
    version = "1.0.0",
    sourceLayer = "managed",
    status = "active",
    available = true,
    reason = "",
    activeRevisionId = revision,
    manageable = true,
  )

  private fun diagnostics(generation: Long) = RuntimeDiagnostics(
    platform = "android",
    capabilities = emptyList(),
    databaseReady = true,
    skillsReady = true,
    modelConfigured = false,
    skillManagementMode = "owner_only",
    activeSnapshotGeneration = generation,
    quarantinedCount = 0,
    lastReloadStatus = "generation:$generation",
  )
}
