package com.generalagent.mobile.ui

import androidx.compose.ui.graphics.Color
import com.generalagent.mobile.runtime.AgentAppTheme
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class GeneralAgentThemeTest {
  @Test
  fun vscodeEightDigitHexKeepsTrailingAlphaChannel() {
    val color = parseVsCodeColor("#ffffff13", Color.Black)

    assertEquals(1f, color.red, 0.001f)
    assertEquals(1f, color.green, 0.001f)
    assertEquals(1f, color.blue, 0.001f)
    assertEquals(0x13 / 255f, color.alpha, 0.001f)
  }

  @Test
  fun themePaletteMapsVscodeWorkbenchColorsToMaterialScheme() {
    val theme = AgentAppTheme(
      id = "test.dark",
      label = "Test Dark",
      type = "dark",
      colors = mapOf(
        "editor.background" to "#101112",
        "foreground" to "#f0f1f2",
        "button.background" to "#123456",
        "sideBar.border" to "#334455",
      ),
    )

    val palette = paletteFor(theme)
    val scheme = colorSchemeFor(palette)

    assertFalse(palette.light)
    assertEquals(parseVsCodeColor("#101112", Color.Black), scheme.background)
    assertEquals(parseVsCodeColor("#f0f1f2", Color.Black), scheme.onBackground)
    assertEquals(parseVsCodeColor("#123456", Color.Black), scheme.primary)
    assertEquals(parseVsCodeColor("#334455", Color.Black), scheme.outline)
  }

  @Test
  fun highContrastLightUsesALightMaterialScheme() {
    val palette = paletteFor(
      AgentAppTheme(
        id = "test.hc-light",
        label = "HC Light",
        type = "hcLight",
        colors = mapOf("editor.background" to "#ffffff"),
      ),
    )

    assertTrue(palette.light)
  }
}
