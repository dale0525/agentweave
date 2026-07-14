package com.agentweave.mobile.ui

import androidx.compose.runtime.staticCompositionLocalOf
import com.agentweave.mobile.runtime.AppStrings

private val EnglishFallback = AppStrings(
  locale = "en",
  messages = mapOf(
    "app.name" to "AgentWeave",
    "app.tagline" to "Ask naturally. The agent will handle the work.",
  ),
)

val LocalAppStrings = staticCompositionLocalOf { EnglishFallback }
