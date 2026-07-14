package com.generalagent.mobile.ui

import android.graphics.Typeface
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.generalagent.mobile.runtime.AgentAppAppearance
import com.generalagent.mobile.runtime.AgentAppFontFiles
import com.generalagent.mobile.runtime.AgentAppTheme
import com.generalagent.mobile.runtime.DEFAULT_ANDROID_THEME_ID

internal data class GaPalette(
  val primary: Color,
  val primaryActive: Color,
  val surface: Color,
  val surfaceMuted: Color,
  val surfaceSubtle: Color,
  val border: Color,
  val text: Color,
  val textSecondary: Color,
  val ready: Color,
  val readyContainer: Color,
  val configured: Color,
  val configuredContainer: Color,
  val amber: Color,
  val amberText: Color,
  val amberContainer: Color,
  val error: Color,
  val errorContainer: Color,
  val primaryText: Color,
  val userMessage: Color,
  val light: Boolean,
)

private val FallbackAppearance = AgentAppAppearance(
  defaultTheme = DEFAULT_ANDROID_THEME_ID,
  themes = listOf(
    AgentAppTheme(
      id = DEFAULT_ANDROID_THEME_ID,
      label = "Dark 2026",
      type = "dark",
      colors = mapOf(
        "editor.background" to "#121314",
        "editor.foreground" to "#BBBEBF",
        "sideBar.background" to "#191A1B",
        "editorWidget.background" to "#202122",
        "sideBar.border" to "#2A2B2C",
        "button.background" to "#297AA0",
        "button.hoverBackground" to "#2B7DA3",
        "button.foreground" to "#FFFFFF",
        "descriptionForeground" to "#8C8C8C",
        "errorForeground" to "#F48771",
      ),
    ),
  ),
)

private val LocalGaPalette = staticCompositionLocalOf { paletteFor(FallbackAppearance.themes.first()) }
val LocalGaMonoFontFamily = staticCompositionLocalOf<FontFamily> { FontFamily.Monospace }

internal val GaPrimary: Color
  @Composable get() = LocalGaPalette.current.primary
internal val GaPrimaryActive: Color
  @Composable get() = LocalGaPalette.current.primaryActive
internal val GaSurface: Color
  @Composable get() = LocalGaPalette.current.surface
internal val GaSurfaceMuted: Color
  @Composable get() = LocalGaPalette.current.surfaceMuted
internal val GaSurfaceSubtle: Color
  @Composable get() = LocalGaPalette.current.surfaceSubtle
internal val GaBorder: Color
  @Composable get() = LocalGaPalette.current.border
internal val GaText: Color
  @Composable get() = LocalGaPalette.current.text
internal val GaTextSecondary: Color
  @Composable get() = LocalGaPalette.current.textSecondary
internal val GaReady: Color
  @Composable get() = LocalGaPalette.current.ready
internal val GaReadyContainer: Color
  @Composable get() = LocalGaPalette.current.readyContainer
internal val GaConfigured: Color
  @Composable get() = LocalGaPalette.current.configured
internal val GaConfiguredContainer: Color
  @Composable get() = LocalGaPalette.current.configuredContainer
internal val GaAmber: Color
  @Composable get() = LocalGaPalette.current.amber
internal val GaAmberText: Color
  @Composable get() = LocalGaPalette.current.amberText
internal val GaAmberContainer: Color
  @Composable get() = LocalGaPalette.current.amberContainer
internal val GaLargeShape = RoundedCornerShape(8.dp)
internal val GaSmallShape = RoundedCornerShape(4.dp)

@Composable
fun GeneralAgentTheme(
  appearance: AgentAppAppearance = FallbackAppearance,
  selectedThemeId: String = appearance.defaultTheme,
  content: @Composable () -> Unit,
) {
  val theme = appearance.themes.firstOrNull { it.id == selectedThemeId }
    ?: appearance.themes.firstOrNull { it.id == appearance.defaultTheme }
    ?: FallbackAppearance.themes.first()
  val palette = paletteFor(theme)
  val fonts = remember(appearance.fonts) { loadFontFamilies(appearance.fonts) }
  CompositionLocalProvider(
    LocalGaPalette provides palette,
    LocalGaMonoFontFamily provides fonts.mono,
  ) {
    MaterialTheme(
      colorScheme = colorSchemeFor(palette),
      typography = typographyFor(fonts.ui, fonts.display),
      content = content,
    )
  }
}

internal fun paletteFor(theme: AgentAppTheme): GaPalette {
  val light = theme.type == "light" || theme.type == "hcLight"
  val fallbackBackground = if (light) "#FFFFFF" else "#1E1E1E"
  val fallbackText = if (light) "#202020" else "#CCCCCC"
  val fallbackSurface = if (light) "#F8F8F8" else "#252526"
  val fallbackMutedSurface = if (light) "#F0F0F0" else "#2D2D2D"
  val fallbackBorder = if (light) "#D4D4D4" else "#3C3C3C"
  val fallbackPrimary = if (light) "#0069CC" else "#0078D4"
  val fallbackMuted = if (light) "#616161" else "#9D9D9D"
  val colors = theme.colors
  fun color(key: String, fallback: String): Color = parseVsCodeColor(colors[key], parseVsCodeColor(fallback, Color.Magenta))
  val surface = color("editor.background", fallbackBackground)
  val surfaceSubtle = color("sideBar.background", colors["panel.background"] ?: fallbackSurface)
  val surfaceMuted = color("editorWidget.background", colors["input.background"] ?: fallbackMutedSurface)
  val border = color(
    "sideBar.border",
    colors["panel.border"] ?: colors["editorWidget.border"] ?: fallbackBorder,
  )
  val primary = color("button.background", colors["focusBorder"] ?: fallbackPrimary)
  return GaPalette(
    primary = primary,
    primaryActive = color("button.hoverBackground", colors["button.background"] ?: fallbackPrimary),
    surface = surface,
    surfaceMuted = surfaceMuted,
    surfaceSubtle = surfaceSubtle,
    border = border,
    text = color("foreground", colors["editor.foreground"] ?: fallbackText),
    textSecondary = color("descriptionForeground", fallbackMuted),
    ready = parseVsCodeColor(if (light) "#059669" else "#4EC9B0", Color.Green),
    readyContainer = parseVsCodeColor(if (light) "#D1FAE5" else "#123B34", Color.Green),
    configured = parseVsCodeColor(if (light) "#2563EB" else "#75BEFF", Color.Blue),
    configuredContainer = parseVsCodeColor(if (light) "#DBEAFE" else "#173A5E", Color.Blue),
    amber = parseVsCodeColor(if (light) "#D97706" else "#CCA700", Color.Yellow),
    amberText = parseVsCodeColor(if (light) "#92400E" else "#F2CC60", Color.Yellow),
    amberContainer = parseVsCodeColor(if (light) "#FEF3C7" else "#3B3012", Color.Yellow),
    error = color("errorForeground", if (light) "#A1260D" else "#F48771"),
    errorContainer = color("inputValidation.errorBackground", if (light) "#FDEDED" else "#3A1D1D"),
    primaryText = color("button.foreground", "#FFFFFF"),
    userMessage = color("chat.requestBubbleBackground", colors["list.inactiveSelectionBackground"] ?: fallbackMutedSurface),
    light = light,
  )
}

internal fun colorSchemeFor(palette: GaPalette): ColorScheme {
  val values: (Boolean) -> ColorScheme = { light ->
    if (light) lightColorScheme(
      primary = palette.primary,
      onPrimary = palette.primaryText,
      primaryContainer = palette.surfaceMuted,
      onPrimaryContainer = palette.text,
      background = palette.surface,
      onBackground = palette.text,
      surface = palette.surface,
      onSurface = palette.text,
      surfaceVariant = palette.surfaceMuted,
      onSurfaceVariant = palette.textSecondary,
      outline = palette.border,
      outlineVariant = palette.border,
      error = palette.error,
      errorContainer = palette.errorContainer,
      onErrorContainer = palette.text,
    ) else darkColorScheme(
      primary = palette.primary,
      onPrimary = palette.primaryText,
      primaryContainer = palette.surfaceMuted,
      onPrimaryContainer = palette.text,
      background = palette.surface,
      onBackground = palette.text,
      surface = palette.surface,
      onSurface = palette.text,
      surfaceVariant = palette.surfaceMuted,
      onSurfaceVariant = palette.textSecondary,
      outline = palette.border,
      outlineVariant = palette.border,
      error = palette.error,
      errorContainer = palette.errorContainer,
      onErrorContainer = palette.text,
    )
  }
  return values(palette.light)
}

internal fun parseVsCodeColor(value: String?, fallback: Color): Color {
  val source = value?.trim()?.removePrefix("#") ?: return fallback
  val expanded = when (source.length) {
    3, 4 -> source.map { "$it$it" }.joinToString("")
    6, 8 -> source
    else -> return fallback
  }
  return runCatching {
    val red = expanded.substring(0, 2)
    val green = expanded.substring(2, 4)
    val blue = expanded.substring(4, 6)
    val alpha = if (expanded.length == 8) expanded.substring(6, 8) else "FF"
    Color("$alpha$red$green$blue".toLong(16).toInt())
  }.getOrDefault(fallback)
}

private data class LoadedFontFamilies(
  val ui: FontFamily,
  val display: FontFamily,
  val mono: FontFamily,
)

private fun loadFontFamilies(files: AgentAppFontFiles): LoadedFontFamilies {
  val ui = loadFont(files.ui) ?: FontFamily.SansSerif
  val display = loadFont(files.display) ?: ui
  val mono = loadFont(files.mono) ?: FontFamily.Monospace
  return LoadedFontFamilies(ui, display, mono)
}

private fun loadFont(file: java.io.File?): FontFamily? =
  file?.let { runCatching { FontFamily(Typeface.createFromFile(it)) }.getOrNull() }

private fun typographyFor(ui: FontFamily, display: FontFamily): Typography {
  val defaults = Typography()
  return Typography(
    displayLarge = defaults.displayLarge.copy(fontFamily = display),
    displayMedium = defaults.displayMedium.copy(fontFamily = display),
    displaySmall = defaults.displaySmall.copy(fontFamily = display),
    headlineLarge = defaults.headlineLarge.copy(fontFamily = display),
    headlineMedium = defaults.headlineMedium.copy(fontFamily = display),
    headlineSmall = TextStyle(
      fontFamily = display,
      fontWeight = FontWeight.Bold,
      fontSize = 20.sp,
      lineHeight = 28.sp,
      letterSpacing = 0.sp,
    ),
    titleLarge = defaults.titleLarge.copy(fontFamily = ui),
    titleMedium = TextStyle(
      fontFamily = ui,
      fontWeight = FontWeight.Bold,
      fontSize = 18.sp,
      lineHeight = 24.sp,
      letterSpacing = 0.sp,
    ),
    titleSmall = defaults.titleSmall.copy(fontFamily = ui),
    bodyLarge = defaults.bodyLarge.copy(fontFamily = ui),
    bodyMedium = TextStyle(
      fontFamily = ui,
      fontWeight = FontWeight.Normal,
      fontSize = 14.sp,
      lineHeight = 20.sp,
      letterSpacing = 0.sp,
    ),
    bodySmall = defaults.bodySmall.copy(fontFamily = ui),
    labelLarge = defaults.labelLarge.copy(fontFamily = ui),
    labelMedium = TextStyle(
      fontFamily = ui,
      fontWeight = FontWeight.Medium,
      fontSize = 12.sp,
      lineHeight = 16.sp,
      letterSpacing = 0.sp,
    ),
    labelSmall = defaults.labelSmall.copy(fontFamily = ui),
  )
}
