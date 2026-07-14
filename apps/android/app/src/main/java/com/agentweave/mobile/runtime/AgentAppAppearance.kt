package com.agentweave.mobile.runtime

import android.content.Context
import java.io.File
import org.json.JSONObject

const val DEFAULT_ANDROID_THEME_ID = "vscode.dark-2026"

data class AgentAppTheme(
  val id: String,
  val label: String,
  val type: String,
  val colors: Map<String, String>,
)

data class AgentAppFontFiles(
  val ui: File? = null,
  val display: File? = null,
  val mono: File? = null,
)

data class AgentAppAppearance(
  val defaultTheme: String,
  val themes: List<AgentAppTheme>,
  val fonts: AgentAppFontFiles = AgentAppFontFiles(),
) {
  fun admittedTheme(themeId: String?): String =
    themeId?.takeIf { candidate -> themes.any { it.id == candidate } } ?: defaultTheme
}

class AndroidAgentAppAppearanceStore(private val context: Context) {
  val appearance: AgentAppAppearance by lazy {
    val root = installedAgentAppRoot(context.filesDir)
    val catalog = runCatching {
      context.assets.open("vscode-themes.json").bufferedReader(Charsets.UTF_8).use { it.readText() }
    }.getOrNull()
    AgentAppAppearanceLoader.load(root, catalog)
  }

  fun selectedTheme(): String {
    val saved = preferences().getString(THEME_PREFERENCE, null)
    return appearance.admittedTheme(saved)
  }

  fun selectTheme(themeId: String): Boolean {
    if (appearance.themes.none { it.id == themeId }) return false
    return preferences().edit().putString(THEME_PREFERENCE, themeId).commit()
  }

  private fun preferences() =
    context.getSharedPreferences(APPEARANCE_PREFERENCES, Context.MODE_PRIVATE)
}

object AgentAppAppearanceLoader {
  fun load(installedRoot: File?, builtinCatalogJson: String?): AgentAppAppearance {
    val catalog = parseCatalog(builtinCatalogJson)
    val root = installedRoot?.takeIf { it.isDirectory }
    val manifest = root?.resolve("agent-app.json")
      ?.takeIf { it.isFile }
      ?.let { file -> runCatching { JSONObject(file.readText(Charsets.UTF_8)) }.getOrNull() }
    val appearance = manifest?.optJSONObject("appearance")
    val themesDocument = appearance?.optJSONObject("themes")

    val builtinIds = themesDocument?.optJSONArray("builtins")?.let { array ->
      (0 until array.length()).mapNotNull { index -> array.optString(index).takeIf(String::isNotBlank) }
    } ?: catalog.map(AgentAppTheme::id)
    val builtinsById = catalog.associateBy(AgentAppTheme::id)
    val builtins = builtinIds.mapNotNull(builtinsById::get)
    val custom = if (root == null) {
      emptyList()
    } else {
      parseCustomThemes(root, themesDocument?.optJSONArray("custom"))
    }
    val selected = (builtins + custom).distinctBy(AgentAppTheme::id)
      .ifEmpty { listOf(fallbackTheme()) }
    val configuredDefault = appearance?.optString("defaultTheme")
      ?.takeIf(String::isNotBlank)
      ?: DEFAULT_ANDROID_THEME_ID
    val defaultTheme = configuredDefault.takeIf { id -> selected.any { it.id == id } }
      ?: selected.firstOrNull { it.id == DEFAULT_ANDROID_THEME_ID }?.id
      ?: selected.first().id
    return AgentAppAppearance(defaultTheme, selected, loadFonts(root))
  }

  private fun parseCatalog(json: String?): List<AgentAppTheme> {
    val document = json?.let { runCatching { JSONObject(it) }.getOrNull() } ?: return listOf(fallbackTheme())
    val themes = document.optJSONArray("themes") ?: return listOf(fallbackTheme())
    return (0 until themes.length()).mapNotNull { index ->
      themes.optJSONObject(index)?.toTheme()?.takeIf { it.id.isNotBlank() }
    }.ifEmpty { listOf(fallbackTheme()) }
  }

  private fun parseCustomThemes(root: File, entries: org.json.JSONArray?): List<AgentAppTheme> {
    if (entries == null) return emptyList()
    return (0 until entries.length()).mapNotNull { index ->
      val entry = entries.optJSONObject(index) ?: return@mapNotNull null
      val id = entry.optString("id").takeIf(String::isNotBlank) ?: return@mapNotNull null
      val relativePath = entry.optString("path").takeIf(String::isNotBlank) ?: return@mapNotNull null
      runCatching {
        val document = loadCustomTheme(root, relativePath)
        val label = entry.optString("label").takeIf(String::isNotBlank)
          ?: document.optString("name").takeIf(String::isNotBlank)
          ?: File(relativePath).nameWithoutExtension
        AgentAppTheme(
          id = id,
          label = label,
          type = normalizeThemeType(document.optString("type"), document.optJSONObject("colors")),
          colors = document.optJSONObject("colors").stringMap(),
        )
      }.getOrNull()
    }
  }

  private fun loadCustomTheme(
    root: File,
    relativePath: String,
    stack: Set<String> = emptySet(),
  ): JSONObject {
    require(stack.size < 8) { "VS Code theme include depth exceeds 8" }
    val canonicalRoot = root.canonicalFile
    val themesRoot = canonicalRoot.resolve("themes").canonicalFile
    val file = canonicalRoot.resolve(relativePath).canonicalFile
    require(file.isFile && file.toPath().startsWith(themesRoot.toPath())) {
      "Custom theme escapes the themes directory"
    }
    require(file.path !in stack) { "VS Code theme include cycle" }
    val document = JSONObject(stripJsonCommentsAndTrailingCommas(file.readText(Charsets.UTF_8)))
    val include = document.optString("include").takeIf(String::isNotBlank) ?: return document
    val included = checkNotNull(file.parentFile).resolve(include).canonicalFile
    require(included.toPath().startsWith(themesRoot.toPath())) {
      "Custom theme include escapes the themes directory"
    }
    val base = loadCustomTheme(
      canonicalRoot,
      included.relativeTo(canonicalRoot).invariantSeparatorsPath,
      stack + file.path,
    )
    val colors = JSONObject(base.optJSONObject("colors")?.toString() ?: "{}")
    document.optJSONObject("colors")?.let { override ->
      override.keys().forEach { key -> colors.put(key, override.get(key)) }
    }
    val merged = JSONObject(base.toString())
    document.keys().forEach { key -> merged.put(key, document.get(key)) }
    merged.put("colors", colors)
    return merged
  }

  private fun loadFonts(root: File?): AgentAppFontFiles {
    val fontsRoot = root?.resolve("fonts")?.takeIf(File::isDirectory) ?: return AgentAppFontFiles()
    val filesBySlot = fontsRoot.listFiles()
      .orEmpty()
      .filter { file -> file.isFile && SUPPORTED_ANDROID_FONT.matches(file.name) }
      .sortedBy { it.name }
      .groupBy { file -> SUPPORTED_ANDROID_FONT.matchEntire(file.name)!!.groupValues[1].lowercase() }
    fun preferred(slot: String): File? = filesBySlot[slot]?.minWithOrNull(
      compareBy<File> { file ->
        val match = SUPPORTED_ANDROID_FONT.matchEntire(file.name)!!
        val weight = match.groupValues[2].ifBlank { "400" }
        val italic = match.groupValues[3].isNotBlank()
        when {
          weight == "400" && !italic -> 0
          !italic -> 1
          else -> 2
        }
      }.thenBy { it.name },
    )
    return AgentAppFontFiles(preferred("ui"), preferred("display"), preferred("mono"))
  }

  private fun JSONObject.toTheme(): AgentAppTheme? {
    val id = optString("id").takeIf(String::isNotBlank) ?: return null
    return AgentAppTheme(
      id = id,
      label = optString("label").takeIf(String::isNotBlank) ?: id,
      type = normalizeThemeType(optString("type"), optJSONObject("colors")),
      colors = optJSONObject("colors").stringMap(),
    )
  }

  private fun normalizeThemeType(type: String, colors: JSONObject?): String {
    if (type in setOf("dark", "light", "hcDark", "hcLight")) return type
    val background = colors?.optString("editor.background")
    val hex = background?.removePrefix("#")?.takeIf { it.length >= 6 }?.take(6) ?: return "dark"
    val channels = runCatching { listOf(0, 2, 4).map { hex.substring(it, it + 2).toInt(16) } }
      .getOrNull() ?: return "dark"
    return if ((channels[0] * 299 + channels[1] * 587 + channels[2] * 114) / 1000 >= 150) {
      "light"
    } else {
      "dark"
    }
  }

  private fun JSONObject?.stringMap(): Map<String, String> {
    if (this == null) return emptyMap()
    return keys().asSequence().mapNotNull { key ->
      optString(key).takeIf(String::isNotBlank)?.let { value -> key to value }
    }.toMap()
  }

  private fun fallbackTheme() = AgentAppTheme(
    id = DEFAULT_ANDROID_THEME_ID,
    label = "Dark 2026",
    type = "dark",
    colors = mapOf(
      "foreground" to "#bfbfbf",
      "descriptionForeground" to "#8C8C8C",
      "focusBorder" to "#3994BCB3",
      "button.background" to "#297AA0",
      "button.foreground" to "#FFFFFF",
      "button.hoverBackground" to "#2B7DA3",
      "input.background" to "#191A1B",
      "sideBar.background" to "#191A1B",
      "sideBar.border" to "#2A2B2CFF",
      "editor.background" to "#121314",
      "editor.foreground" to "#BBBEBF",
      "editorWidget.background" to "#202122",
      "errorForeground" to "#f48771",
    ),
  )
}

internal fun installedAgentAppRoot(filesDir: File): File? {
  val appRoot = filesDir.resolve("agent-app")
  val revision = appRoot.resolve("current").takeIf(File::isFile)
    ?.readText(Charsets.UTF_8)?.trim()
    ?.takeIf { HASH_PATTERN.matches(it) }
    ?: return null
  val revisionsRoot = appRoot.resolve("revisions").canonicalFile
  val installed = revisionsRoot.resolve(revision).canonicalFile
  return installed.takeIf {
    it.isDirectory && it.toPath().startsWith(revisionsRoot.toPath()) && it.resolve("agent-app.json").isFile
  }
}

internal fun stripJsonCommentsAndTrailingCommas(text: String): String {
  val withoutComments = StringBuilder(text.length)
  var index = 0
  var inString = false
  var escaped = false
  while (index < text.length) {
    val current = text[index]
    val next = text.getOrNull(index + 1)
    when {
      inString -> {
        withoutComments.append(current)
        if (escaped) escaped = false
        else if (current == '\\') escaped = true
        else if (current == '"') inString = false
        index += 1
      }
      current == '"' -> {
        inString = true
        withoutComments.append(current)
        index += 1
      }
      current == '/' && next == '/' -> {
        withoutComments.append("  ")
        index += 2
        while (index < text.length && text[index] != '\n') {
          withoutComments.append(' ')
          index += 1
        }
      }
      current == '/' && next == '*' -> {
        withoutComments.append("  ")
        index += 2
        while (index < text.length && !(text[index] == '*' && text.getOrNull(index + 1) == '/')) {
          withoutComments.append(if (text[index] == '\n') '\n' else ' ')
          index += 1
        }
        if (index < text.length) {
          withoutComments.append("  ")
          index += 2
        }
      }
      else -> {
        withoutComments.append(current)
        index += 1
      }
    }
  }

  val output = StringBuilder(withoutComments.length)
  index = 0
  inString = false
  escaped = false
  while (index < withoutComments.length) {
    val current = withoutComments[index]
    if (inString) {
      output.append(current)
      if (escaped) escaped = false
      else if (current == '\\') escaped = true
      else if (current == '"') inString = false
    } else if (current == '"') {
      inString = true
      output.append(current)
    } else if (current == ',') {
      var lookahead = index + 1
      while (lookahead < withoutComments.length && withoutComments[lookahead].isWhitespace()) lookahead += 1
      if (lookahead >= withoutComments.length || withoutComments[lookahead] !in setOf('}', ']')) {
        output.append(current)
      }
    } else {
      output.append(current)
    }
    index += 1
  }
  return output.toString()
}

private const val APPEARANCE_PREFERENCES = "agentweave.appearance.v1"
private const val THEME_PREFERENCE = "theme"
private val HASH_PATTERN = Regex("^[0-9a-f]{64}$")
private val SUPPORTED_ANDROID_FONT = Regex(
  "^(ui|display|mono)(?:-(100|200|300|400|500|600|700|800|900))?(?:-(italic))?\\.(ttf|otf)$",
  RegexOption.IGNORE_CASE,
)
