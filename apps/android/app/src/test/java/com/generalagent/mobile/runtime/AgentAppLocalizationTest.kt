package com.generalagent.mobile.runtime

import java.nio.file.Files
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class AgentAppLocalizationTest {
  @Test
  fun loadsDeclaredLocalesAndMergesHostWithAppMessages() {
    val root = Files.createTempDirectory("agent-app-localization").toFile()
    try {
      root.resolve("locales").mkdirs()
      root.resolve("agent-app.json").writeText(
        """
        {
          "localization": {
            "defaultLocale": "zh-CN",
            "locales": [
              {"id":"en","label":"English","resource":"locales/en.json"},
              {"id":"zh-CN","label":"简体中文","resource":"locales/zh-CN.json"}
            ]
          }
        }
        """.trimIndent(),
        Charsets.UTF_8,
      )
      root.resolve("locales/en.json").writeText("""{"app.name":"Example"}""", Charsets.UTF_8)
      root.resolve("locales/zh-CN.json").writeText("""{"app.name":"示例"}""", Charsets.UTF_8)
      val host = mapOf(
        "en" to """{"settings.title":"Settings","app.name":"GeneralAgent"}""",
        "zh-CN" to """{"settings.title":"设置"}""",
      )

      val localization = AgentAppLocalizationLoader.load(root, host::get)

      assertEquals("zh-CN", localization.defaultLocale)
      assertEquals(listOf("en", "zh-CN"), localization.locales.map(AgentAppLocale::id))
      assertEquals("示例", localization.strings("zh-CN").text("app.name"))
      assertEquals("设置", localization.strings("zh-CN").text("settings.title"))
      assertEquals("Settings", localization.strings("en").text("settings.title"))
    } finally {
      root.deleteRecursively()
    }
  }

  @Test
  fun fallsBackToEnglishAndPreservesUnknownPlaceholders() {
    val localization = AgentAppLocalizationLoader.load(null) { locale ->
      if (locale == "en") """{"message":"Hello {name} {missing}"}""" else null
    }

    assertEquals("en", localization.defaultLocale)
    assertEquals(
      "Hello Logic {missing}",
      localization.strings("missing").text("message", mapOf("name" to "Logic")),
    )
    assertTrue(localization.locales.any { it.id == "zh-CN" })
  }
}
