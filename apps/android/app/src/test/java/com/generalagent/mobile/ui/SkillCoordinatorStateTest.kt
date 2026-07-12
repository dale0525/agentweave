package com.generalagent.mobile.ui

import com.generalagent.mobile.runtime.RuntimeDiagnostics
import com.generalagent.mobile.runtime.RuntimeSkill
import com.generalagent.mobile.runtime.RuntimeSkillDetail
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SkillCoordinatorStateTest {
  @Test
  fun parentSnapshotRecompositionPreservesVisibleDetail() {
    val state = coordinatorState().copy(
      route = SkillRoute.Detail("com.example.owner"),
      detail = detail("revision-active"),
    )

    val updated = reduceSkillCoordinator(
      state,
      SkillCoordinatorEvent.ParentSnapshotUpdated(
        inventory = listOf(skill("revision-new")),
        diagnostics = diagnostics(8),
      ),
    )

    assertEquals(SkillRoute.Detail("com.example.owner"), updated.route)
    assertEquals("revision-active", updated.detail?.activeRevisionId)
    assertEquals(8L, updated.diagnostics.activeSnapshotGeneration)
  }

  @Test
  fun lateCompletionAfterBackIsIgnored() {
    val started = reduceSkillCoordinator(
      coordinatorState().copy(route = SkillRoute.Detail("com.example.owner")),
      SkillCoordinatorEvent.OperationStarted("detail-load"),
    )
    val token = checkNotNull(started.operationToken)
    val afterBack = reduceSkillCoordinator(started, SkillCoordinatorEvent.Navigate(SkillRoute.ListRoute))

    val late = reduceSkillCoordinator(
      afterBack,
      SkillCoordinatorEvent.DetailLoaded(token, detail("late-revision")),
    )

    assertEquals(SkillRoute.ListRoute, late.route)
    assertEquals(null, late.detail)
    assertFalse(late.busy)
  }

  @Test
  fun publicationRefreshKeepsDetailOrFallsBackToList() {
    val state = coordinatorState().copy(
      route = SkillRoute.Detail("com.example.owner"),
      detail = detail("revision-active"),
    )
    val refreshed = reduceSkillCoordinator(
      state,
      SkillCoordinatorEvent.PublicationRefreshed(
        inventory = listOf(skill("revision-new")),
        diagnostics = diagnostics(8),
        detail = detail("revision-new"),
      ),
    )
    assertEquals("revision-new", refreshed.detail?.activeRevisionId)
    assertTrue(refreshed.route is SkillRoute.Detail)

    val removed = reduceSkillCoordinator(
      refreshed,
      SkillCoordinatorEvent.PublicationRefreshed(emptyList(), diagnostics(9), detail = null),
    )
    assertEquals(SkillRoute.ListRoute, removed.route)
    assertFalse(removed.immersive)
  }

  @Test
  fun initialLoadFailureIsRetainedAndRetryable() {
    val failed = reduceSkillCoordinator(
      coordinatorState(),
      SkillCoordinatorEvent.InitialLoadFailed("Managed inventory unavailable"),
    )

    assertEquals("Managed inventory unavailable", failed.inlineError)
    assertTrue(failed.inventory.isNotEmpty())
    assertFalse(failed.busy)
    assertTrue(failed.retryAvailable)
  }

  private fun coordinatorState() = SkillCoordinatorState(
    inventory = listOf(skill("revision-active")),
    diagnostics = diagnostics(7),
  )

  private fun skill(revision: String) = RuntimeSkill(
    packageId = "com.example.owner",
    displayName = "Owner skill",
    version = "1.0.0",
    sourceLayer = "managed",
    status = "active",
    available = true,
    reason = "",
    activeRevisionId = revision,
    manageable = true,
  )

  private fun detail(revision: String): RuntimeSkillDetail =
    RuntimeSkillDetail(
      packageId = "com.example.owner",
      displayName = "Owner skill",
      version = "1.0.0",
      sourceLayer = "managed",
      status = "active",
      reason = "",
      activeRevisionId = revision,
      revisions = emptyList(),
      editableDraft = null,
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
