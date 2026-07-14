package com.generalagent.mobile.ui

import androidx.compose.runtime.staticCompositionLocalOf
import com.generalagent.mobile.runtime.AppStrings

private val EnglishFallback = AppStrings(
  locale = "en",
  messages = mapOf(
    "app.name" to "GeneralAgent",
    "app.tagline" to "Ask naturally. The agent will handle the work.",
  ),
)

val LocalAppStrings = staticCompositionLocalOf { EnglishFallback }
