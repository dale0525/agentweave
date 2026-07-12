package com.generalagent.mobile.ui

import com.generalagent.mobile.runtime.RuntimeDiagnostics
import com.generalagent.mobile.runtime.RuntimeSkill
import com.generalagent.mobile.runtime.RuntimeSkillDetail

sealed interface SkillRoute {
  data object ListRoute : SkillRoute
  data class Detail(val packageId: String) : SkillRoute
  data class Draft(val packageId: String?, val creating: Boolean) : SkillRoute
}

data class SkillCoordinatorState(
  val inventory: List<RuntimeSkill>,
  val diagnostics: RuntimeDiagnostics,
  val route: SkillRoute = SkillRoute.ListRoute,
  val detail: RuntimeSkillDetail? = null,
  val inlineError: String? = null,
  val busy: Boolean = false,
  val retryAvailable: Boolean = false,
  val operationToken: Long? = null,
  val nextToken: Long = 1,
) {
  val immersive: Boolean get() = route !is SkillRoute.ListRoute
}

sealed interface SkillCoordinatorEvent {
  data class ParentSnapshotUpdated(
    val inventory: List<RuntimeSkill>,
    val diagnostics: RuntimeDiagnostics,
  ) : SkillCoordinatorEvent

  data class Navigate(val route: SkillRoute) : SkillCoordinatorEvent
  data class OperationStarted(val operation: String) : SkillCoordinatorEvent
  data class DetailLoaded(val token: Long, val detail: RuntimeSkillDetail) : SkillCoordinatorEvent
  data class PublicationRefreshed(
    val inventory: List<RuntimeSkill>,
    val diagnostics: RuntimeDiagnostics,
    val detail: RuntimeSkillDetail?,
  ) : SkillCoordinatorEvent

  data class InitialLoadFailed(val message: String) : SkillCoordinatorEvent
}

fun reduceSkillCoordinator(
  state: SkillCoordinatorState,
  event: SkillCoordinatorEvent,
): SkillCoordinatorState =
  when (event) {
    is SkillCoordinatorEvent.ParentSnapshotUpdated -> state.copy(
      inventory = event.inventory,
      diagnostics = event.diagnostics,
    )
    is SkillCoordinatorEvent.Navigate -> state.copy(
      route = event.route,
      detail = if (event.route is SkillRoute.ListRoute) null else state.detail,
      busy = false,
      operationToken = null,
      nextToken = state.nextToken + 1,
      inlineError = null,
    )
    is SkillCoordinatorEvent.OperationStarted -> state.copy(
      busy = true,
      operationToken = state.nextToken,
      nextToken = state.nextToken + 1,
      inlineError = null,
    )
    is SkillCoordinatorEvent.DetailLoaded -> if (event.token != state.operationToken) {
      state
    } else {
      state.copy(
        route = SkillRoute.Detail(event.detail.packageId),
        detail = event.detail,
        busy = false,
        operationToken = null,
      )
    }
    is SkillCoordinatorEvent.PublicationRefreshed -> {
      val keepDetail = state.route is SkillRoute.Detail && event.detail != null
      state.copy(
        inventory = event.inventory,
        diagnostics = event.diagnostics,
        route = if (keepDetail) state.route else SkillRoute.ListRoute,
        detail = if (keepDetail) event.detail else null,
        busy = false,
        operationToken = null,
        inlineError = null,
      )
    }
    is SkillCoordinatorEvent.InitialLoadFailed -> state.copy(
      busy = false,
      operationToken = null,
      inlineError = event.message,
      retryAvailable = true,
    )
  }
