package com.generalagent.mobile.runtime

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class AgentAppAppearanceTest {
  @get:Rule
  val temporaryFolder = TemporaryFolder()

  @Test
  fun manifestSelectsPackagedBuiltinsAndCustomTheme() {
    val root = temporaryFolder.newFolder("agent-app")
    root.resolve("themes").mkdir()
    root.resolve("themes/base.json").writeText(
      """{"type":"light","colors":{"editor.background":"#ffffff","foreground":"#222222"}}""",
      Charsets.UTF_8,
    )
    root.resolve("themes/brand.jsonc").writeText(
      """
        {
          // Android accepts the same JSONC theme source as desktop.
          "include": "base.json",
          "name": "Brand Light",
          "colors": { "button.background": "#005fb8", },
        }
      """.trimIndent(),
      Charsets.UTF_8,
    )
    root.resolve("agent-app.json").writeText(
      """
        {
          "appearance": {
            "defaultTheme": "com.example.brand",
            "themes": {
              "builtins": ["vscode.light-2026"],
              "custom": [{
                "id": "com.example.brand",
                "label": "Custom Brand",
                "path": "themes/brand.jsonc"
              }]
            }
          }
        }
      """.trimIndent(),
      Charsets.UTF_8,
    )

    val loaded = AgentAppAppearanceLoader.load(root, TEST_CATALOG)

    assertEquals("com.example.brand", loaded.defaultTheme)
    assertEquals(listOf("vscode.light-2026", "com.example.brand"), loaded.themes.map { it.id })
    val custom = loaded.themes.last()
    assertEquals("Custom Brand", custom.label)
    assertEquals("light", custom.type)
    assertEquals("#ffffff", custom.colors["editor.background"])
    assertEquals("#005fb8", custom.colors["button.background"])
  }

  @Test
  fun invalidSavedSelectionFallsBackToDark2026() {
    val loaded = AgentAppAppearanceLoader.load(null, TEST_CATALOG)

    assertEquals(DEFAULT_ANDROID_THEME_ID, loaded.defaultTheme)
    assertEquals(DEFAULT_ANDROID_THEME_ID, loaded.admittedTheme("missing.theme"))
    assertEquals("vscode.light-2026", loaded.admittedTheme("vscode.light-2026"))
  }

  @Test
  fun androidFontSlotsPreferNormal400TtfOrOtfAndIgnoreWebFonts() {
    val root = temporaryFolder.newFolder("font-app")
    root.resolve("agent-app.json").writeText("{}", Charsets.UTF_8)
    val fonts = root.resolve("fonts").apply { mkdir() }
    fonts.resolve("ui-700.ttf").writeBytes(byteArrayOf(1))
    fonts.resolve("ui-400.ttf").writeBytes(byteArrayOf(2))
    fonts.resolve("display.otf").writeBytes(byteArrayOf(3))
    fonts.resolve("mono.woff2").writeBytes(byteArrayOf(4))

    val loaded = AgentAppAppearanceLoader.load(root, TEST_CATALOG)

    assertEquals("ui-400.ttf", loaded.fonts.ui?.name)
    assertEquals("display.otf", loaded.fonts.display?.name)
    assertNull(loaded.fonts.mono)
  }

  @Test
  fun installedRootRequiresAConfinedHashRevision() {
    val filesDir = temporaryFolder.newFolder("files")
    val appRoot = filesDir.resolve("agent-app").apply { mkdir() }
    val revisions = appRoot.resolve("revisions").apply { mkdir() }
    val hash = "a".repeat(64)
    val revision = revisions.resolve(hash).apply { mkdir() }
    revision.resolve("agent-app.json").writeText("{}", Charsets.UTF_8)
    appRoot.resolve("current").writeText("$hash\n", Charsets.UTF_8)

    assertEquals(revision.canonicalFile, installedAgentAppRoot(filesDir))

    appRoot.resolve("current").writeText("../../outside", Charsets.UTF_8)
    assertNull(installedAgentAppRoot(filesDir))
    assertTrue(revision.isDirectory)
    assertFalse(File(filesDir, "outside").exists())
  }
}

private val TEST_CATALOG =
  """
    {
      "themes": [
        {
          "id": "vscode.dark-2026",
          "label": "Dark 2026",
          "type": "dark",
          "colors": {"editor.background":"#121314","editor.foreground":"#BBBEBF"}
        },
        {
          "id": "vscode.light-2026",
          "label": "Light 2026",
          "type": "light",
          "colors": {"editor.background":"#FFFFFF","editor.foreground":"#202020"}
        }
      ]
    }
  """.trimIndent()
