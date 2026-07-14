package com.generalagent.mobile.runtime

import android.content.res.AssetManager
import java.io.File
import java.io.FileOutputStream
import java.io.InputStream
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.MessageDigest
import java.util.UUID

interface AgentAppAssetSource {
  fun isAvailable(): Boolean

  fun contentHash(): String

  fun files(): List<String>

  fun open(relativePath: String): InputStream
}

class AndroidAgentAppAssetSource(
  private val assets: AssetManager,
  private val assetRoot: String = "agent-app",
) : AgentAppAssetSource {
  override fun isAvailable(): Boolean = assets.list(assetRoot)?.isNotEmpty() == true

  override fun contentHash(): String =
    assets.open("$assetRoot/app.sha256")
      .bufferedReader(StandardCharsets.UTF_8)
      .use { it.readText().trim() }

  override fun files(): List<String> {
    val files = mutableListOf<String>()
    collect("$assetRoot/package", "", files)
    return files.sorted()
  }

  override fun open(relativePath: String): InputStream =
    assets.open("$assetRoot/package/$relativePath", AssetManager.ACCESS_STREAMING)

  private fun collect(assetPath: String, relativePath: String, output: MutableList<String>) {
    val children = assets.list(assetPath)?.sorted().orEmpty()
    if (children.isEmpty()) {
      require(relativePath.isNotEmpty()) { "Agent App asset package is empty" }
      output += relativePath
      return
    }
    children.forEach { child ->
      collect(
        "$assetPath/$child",
        if (relativePath.isEmpty()) child else "$relativePath/$child",
        output,
      )
    }
  }
}

class AgentAppAssetInstaller(
  private val filesDir: File,
  private val source: AgentAppAssetSource,
) {
  fun install(): File? {
    if (!source.isAvailable()) return null
    val expected = source.contentHash()
    require(HASH_PATTERN.matches(expected)) { "Agent App asset hash is invalid" }
    val files = source.files().onEach(::validateRelativePath)
    require(files.isNotEmpty()) { "Agent App asset package is empty" }
    require(files.contains("agent-app.json")) { "Agent App manifest asset is missing" }

    val root = filesDir.resolve("agent-app")
    val revisions = root.resolve("revisions")
    checkDirectory(root)
    checkDirectory(revisions)
    val target = revisions.resolve(expected)
    if (target.isDirectory) {
      check(hashTree(target) == expected) { "Installed Agent App asset hash mismatch" }
      return target
    }

    val staging = revisions.resolve(".incoming-${UUID.randomUUID()}")
    check(staging.mkdir()) { "Unable to create Agent App staging directory" }
    try {
      files.forEach { relativePath ->
        val destination = staging.resolve(relativePath)
        val parent = checkNotNull(destination.parentFile)
        check(parent.isDirectory || parent.mkdirs()) {
          "Unable to prepare Agent App asset directory"
        }
        source.open(relativePath).use { input ->
          FileOutputStream(destination).use { output ->
            input.copyTo(output)
            output.fd.sync()
          }
        }
      }
      check(hashTree(staging) == expected) { "Staged Agent App asset hash mismatch" }
      Files.move(
        staging.toPath(),
        target.toPath(),
        StandardCopyOption.ATOMIC_MOVE,
      )
      FileOutputStream(root.resolve("current"), false).use { output ->
        output.write("$expected\n".toByteArray(StandardCharsets.UTF_8))
        output.fd.sync()
      }
      return target
    } finally {
      if (staging.exists()) staging.deleteRecursively()
    }
  }

  private fun checkDirectory(directory: File) {
    if (directory.exists()) {
      check(directory.isDirectory && !Files.isSymbolicLink(directory.toPath())) {
        "Agent App asset root must be a real directory"
      }
    } else {
      check(directory.mkdir()) { "Unable to create Agent App asset directory" }
    }
  }
}

private fun hashTree(root: File): String {
  val digest = MessageDigest.getInstance("SHA-256")
  root.walkTopDown()
    .filter(File::isFile)
    .sortedBy { it.relativeTo(root).invariantSeparatorsPath }
    .forEach { file ->
      val relative = file.relativeTo(root).invariantSeparatorsPath
      val bytes = file.readBytes()
      digest.update(relative.toByteArray(StandardCharsets.UTF_8))
      digest.update(byteArrayOf(0))
      digest.update(bytes.size.toString().toByteArray(StandardCharsets.US_ASCII))
      digest.update(byteArrayOf(0))
      digest.update(bytes)
    }
  return digest.digest().joinToString("") { byte -> "%02x".format(byte) }
}

private fun validateRelativePath(path: String) {
  require(path.isNotBlank()) { "Agent App asset path is required" }
  require(!path.startsWith('/') && '\\' !in path) { "Agent App asset path is invalid" }
  require(path.split('/').none { it.isBlank() || it == "." || it == ".." }) {
    "Agent App asset path escapes its package"
  }
}

private val HASH_PATTERN = Regex("^[0-9a-f]{64}$")
