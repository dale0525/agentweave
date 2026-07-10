package com.generalagent.mobile.ui

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

internal val GaPrimary = Color(0xFF0D9488)
internal val GaPrimaryActive = Color(0xFF008378)
internal val GaSurface = Color(0xFFFFFFFF)
internal val GaSurfaceMuted = Color(0xFFF4F4F5)
internal val GaSurfaceSubtle = Color(0xFFF9FAFB)
internal val GaBorder = Color(0xFFE5E7EB)
internal val GaText = Color(0xFF1A1A1A)
internal val GaTextSecondary = Color(0xFF5F6368)
internal val GaReady = Color(0xFF059669)
internal val GaReadyContainer = Color(0xFFD1FAE5)
internal val GaConfigured = Color(0xFF2563EB)
internal val GaConfiguredContainer = Color(0xFFDBEAFE)
internal val GaAmber = Color(0xFFD97706)
internal val GaAmberText = Color(0xFF92400E)
internal val GaAmberContainer = Color(0xFFFEF3C7)
internal val GaLargeShape = RoundedCornerShape(8.dp)
internal val GaSmallShape = RoundedCornerShape(4.dp)

private val GaColorScheme = lightColorScheme(
  primary = GaPrimary,
  onPrimary = Color.White,
  primaryContainer = GaPrimaryActive,
  onPrimaryContainer = Color.White,
  background = GaSurface,
  onBackground = GaText,
  surface = GaSurface,
  onSurface = GaText,
  surfaceVariant = GaSurfaceMuted,
  onSurfaceVariant = GaTextSecondary,
  outline = GaBorder,
  error = Color(0xFFBA1A1A),
)

private val GaTypography = Typography(
  headlineSmall = TextStyle(
    fontFamily = FontFamily.SansSerif,
    fontWeight = FontWeight.Bold,
    fontSize = 20.sp,
    lineHeight = 28.sp,
    letterSpacing = 0.sp,
  ),
  titleMedium = TextStyle(
    fontFamily = FontFamily.SansSerif,
    fontWeight = FontWeight.Bold,
    fontSize = 18.sp,
    lineHeight = 24.sp,
    letterSpacing = 0.sp,
  ),
  bodyMedium = TextStyle(
    fontFamily = FontFamily.SansSerif,
    fontWeight = FontWeight.Normal,
    fontSize = 14.sp,
    lineHeight = 20.sp,
    letterSpacing = 0.sp,
  ),
  labelMedium = TextStyle(
    fontFamily = FontFamily.SansSerif,
    fontWeight = FontWeight.Medium,
    fontSize = 12.sp,
    lineHeight = 16.sp,
    letterSpacing = 0.sp,
  ),
)

@Composable
fun GeneralAgentTheme(content: @Composable () -> Unit) {
  MaterialTheme(
    colorScheme = GaColorScheme,
    typography = GaTypography,
    content = content,
  )
}
