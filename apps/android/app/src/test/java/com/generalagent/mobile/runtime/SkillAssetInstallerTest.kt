package com.generalagent.mobile.runtime

import java.io.ByteArrayInputStream
import java.io.File
import java.io.InputStream
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.security.MessageDigest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class SkillAssetInstallerTest {
  @get:Rule
  val temporaryFolder = TemporaryFolder()

  @Test
  fun installsBundleOnlyWhenHashChanges() {
    val files = validBundleFiles("v1")
    val assets = FakeSkillAssets(files)
    val installer = SkillAssetInstaller(temporaryFolder.root, assets)

    val first = installer.installVerifiedBundle()
    val second = installer.installVerifiedBundle()

    assertTrue(first.changed)
    assertFalse(second.changed)
    assertEquals(bundleHash(files), second.contentHash)
    assertEquals(first.root.canonicalFile, second.root.canonicalFile)
  }

  @Test
  fun retainsPublishedRevisionsWhenHashChanges() {
    val firstFiles = validBundleFiles("v1")
    val secondFiles = validBundleFiles("v2")
    val first = SkillAssetInstaller(temporaryFolder.root, FakeSkillAssets(firstFiles))
      .installVerifiedBundle()
    val second = SkillAssetInstaller(temporaryFolder.root, FakeSkillAssets(secondFiles))
      .installVerifiedBundle()

    assertTrue(first.root.isDirectory)
    assertTrue(second.root.isDirectory)
    assertFalse(first.root.canonicalFile == second.root.canonicalFile)
    assertEquals(second.contentHash, currentHash(temporaryFolder.root))
  }

  @Test
  fun rejectsAssetTraversalEntries() {
    val files = validBundleFiles("v1") + ("../escape" to byteArrayOf(1))

    assertThrows(IllegalArgumentException::class.java) {
      SkillAssetInstaller(temporaryFolder.root, FakeSkillAssets(files)).installVerifiedBundle()
    }

    assertFalse(temporaryFolder.root.resolve("escape").exists())
  }

  @Test
  fun rejectsSymlinkAndSpecialAssetEntries() {
    for (type in listOf(SkillAssetType.SYMLINK, SkillAssetType.SPECIAL)) {
      val assets = FakeSkillAssets(
        files = validBundleFiles("v1"),
        extraEntries = listOf(SkillAssetEntry("packages/unsafe", type)),
      )

      assertThrows(IllegalArgumentException::class.java) {
        SkillAssetInstaller(temporaryFolder.root, assets).installVerifiedBundle()
      }
    }
  }

  @Test
  fun rejectsSymlinkedInstallerLockWithoutTouchingTarget() {
    val bundleRoot = temporaryFolder.root.resolve("builtin-skills")
    assertTrue(bundleRoot.mkdirs())
    val outside = temporaryFolder.newFile("outside-lock")
    outside.writeText("unchanged", Charsets.UTF_8)
    Files.createSymbolicLink(bundleRoot.resolve(".install.lock").toPath(), outside.toPath())

    assertThrows(IllegalStateException::class.java) {
      SkillAssetInstaller(temporaryFolder.root, FakeSkillAssets(validBundleFiles("v1")))
        .installVerifiedBundle()
    }

    assertEquals("unchanged", outside.readText(Charsets.UTF_8))
  }

  @Test
  fun rejectsBundleWhoseDeclaredHashDoesNotMatchContent() {
    val files = validBundleFiles("v1")
    val assets = FakeSkillAssets(files, declaredHash = "0".repeat(64))

    assertThrows(IllegalStateException::class.java) {
      SkillAssetInstaller(temporaryFolder.root, assets).installVerifiedBundle()
    }

    assertFalse(temporaryFolder.root.resolve("builtin-skills/current").exists())
  }

  @Test
  fun failedCopyPreservesLastKnownGoodCurrentRevision() {
    val goodFiles = validBundleFiles("v1")
    val first = SkillAssetInstaller(temporaryFolder.root, FakeSkillAssets(goodFiles))
      .installVerifiedBundle()
    val badFiles = validBundleFiles("v2") + ("packages/fail" to byteArrayOf(9))
    val failing = FakeSkillAssets(badFiles, failOnOpen = "packages/fail")

    assertThrows(IllegalStateException::class.java) {
      SkillAssetInstaller(temporaryFolder.root, failing).installVerifiedBundle()
    }

    assertEquals(first.contentHash, currentHash(temporaryFolder.root))
    assertTrue(first.root.isDirectory)
    assertFalse(
      temporaryFolder.root.resolve("builtin-skills/revisions/.${bundleHash(badFiles)}.incoming").exists(),
    )
  }
}

private class FakeSkillAssets(
  private val files: Map<String, ByteArray>,
  private val declaredHash: String = bundleHash(files),
  private val extraEntries: List<SkillAssetEntry> = emptyList(),
  private val failOnOpen: String? = null,
) : SkillAssetSource {
  override fun bundleHash(): String = declaredHash

  override fun entries(): List<SkillAssetEntry> =
    files.keys.map { SkillAssetEntry(it, SkillAssetType.FILE) } + extraEntries

  override fun open(relativePath: String): InputStream {
    check(relativePath != failOnOpen) { "injected asset read failure" }
    return ByteArrayInputStream(checkNotNull(files[relativePath]))
  }
}

private fun validBundleFiles(version: String): Map<String, ByteArray> =
  mapOf(
    "current" to "{\"schemaVersion\":2,\"active\":{\"generation\":\"$version\"}}"
      .toByteArray(StandardCharsets.UTF_8),
    "generations/$version/skill-bundle.json" to "{\"schemaVersion\":1,\"packages\":[]}"
      .toByteArray(StandardCharsets.UTF_8),
    "generations/$version/skill-bundle.lock" to "{\"schemaVersion\":1,\"packages\":[]}"
      .toByteArray(StandardCharsets.UTF_8),
  )

private fun currentHash(filesDir: File): String =
  filesDir.resolve("builtin-skills/current").readText(Charsets.UTF_8).trim()

private fun bundleHash(files: Map<String, ByteArray>): String {
  val digest = MessageDigest.getInstance("SHA-256")
  for ((path, bytes) in files.toSortedMap()) {
    digest.update(path.toByteArray(StandardCharsets.UTF_8))
    digest.update(0)
    digest.update(bytes.size.toString().toByteArray(StandardCharsets.US_ASCII))
    digest.update(0)
    digest.update(bytes)
  }
  return digest.digest().joinToString("") { byte -> "%02x".format(byte) }
}
