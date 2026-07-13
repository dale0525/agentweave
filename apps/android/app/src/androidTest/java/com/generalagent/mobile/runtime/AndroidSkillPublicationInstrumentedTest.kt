package com.generalagent.mobile.runtime

import androidx.test.platform.app.InstrumentationRegistry
import java.io.ByteArrayInputStream
import java.io.File
import java.io.InputStream
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.security.MessageDigest
import java.util.UUID
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class AndroidSkillPublicationInstrumentedTest {
  @Test
  fun boundedRetentionSupportsUpdateAndDowngradeWithoutTouchingForeignEntries() {
    val root = testRoot("bounded-retention")
    val firstFiles = bundleFiles("android-v1")
    val secondFiles = bundleFiles("android-v2")
    val thirdFiles = bundleFiles("android-v3")
    val first = SkillAssetInstaller(root, InstrumentedSkillAssets(firstFiles)).installVerifiedBundle()
    val second = SkillAssetInstaller(root, InstrumentedSkillAssets(secondFiles)).installVerifiedBundle()
    val revisions = root.resolve("builtin-skills/revisions")
    val foreign = revisions.resolve("foreign-owner-data").apply { check(mkdir()) }
    val outside = root.resolve("outside-retention").apply { check(mkdir()) }
    val symlink = revisions.resolve("f".repeat(64)).toPath()
    Files.createSymbolicLink(symlink, outside.toPath())

    val third = SkillAssetInstaller(root, InstrumentedSkillAssets(thirdFiles)).installVerifiedBundle()

    assertFalse(first.root.exists())
    assertEquals(setOf(third.contentHash, second.contentHash), revisionHashes(root))
    assertEquals(second.contentHash, previousHash(root))
    assertTrue(foreign.isDirectory)
    assertTrue(Files.isSymbolicLink(symlink))
    assertTrue(outside.isDirectory)

    SkillAssetInstaller(root, InstrumentedSkillAssets(secondFiles)).installVerifiedBundle()

    assertEquals(second.contentHash, currentHash(root))
    assertEquals(third.contentHash, previousHash(root))
    assertEquals(setOf(second.contentHash, third.contentHash), revisionHashes(root))
    root.deleteRecursively()
  }

  @Test
  fun retryFinishesCleanupInterruptedAfterCurrentIsDurable() {
    val root = testRoot("interrupted-retention")
    val firstFiles = bundleFiles("cleanup-v1")
    val secondFiles = bundleFiles("cleanup-v2")
    val thirdFiles = bundleFiles("cleanup-v3")
    SkillAssetInstaller(root, InstrumentedSkillAssets(firstFiles)).installVerifiedBundle()
    val second = SkillAssetInstaller(root, InstrumentedSkillAssets(secondFiles)).installVerifiedBundle()
    val thirdHash = bundleHash(thirdFiles)
    val interrupted = SkillAssetInstaller(
      root,
      InstrumentedSkillAssets(thirdFiles),
      AndroidSkillPublicationFileSystem(),
      SkillPublicationFaults { point ->
        if (point == SkillPublicationFaultPoint.BUNDLE_ROOT_SYNCED) {
          throw IllegalStateException("injected interruption before retention cleanup")
        }
      },
    )

    assertThrows(IllegalStateException::class.java) { interrupted.installVerifiedBundle() }
    assertEquals(thirdHash, currentHash(root))
    assertEquals(second.contentHash, previousHash(root))
    assertEquals(3, revisionHashes(root).size)

    SkillAssetInstaller(root, InstrumentedSkillAssets(thirdFiles)).installVerifiedBundle()

    assertEquals(setOf(thirdHash, second.contentHash), revisionHashes(root))
    root.deleteRecursively()
  }

  @Test
  fun previousPointerCrashRecoversAcrossRetryDowngradeAndReupgrade() {
    val root = testRoot("previous-journal")
    val rollbackFiles = bundleFiles("android-journal-v0")
    val currentFiles = bundleFiles("android-journal-v1")
    val targetFiles = bundleFiles("android-journal-v2")
    val rollback = SkillAssetInstaller(root, InstrumentedSkillAssets(rollbackFiles)).installVerifiedBundle()
    val current = SkillAssetInstaller(root, InstrumentedSkillAssets(currentFiles)).installVerifiedBundle()
    val revisions = root.resolve("builtin-skills/revisions")
    val foreign = revisions.resolve("foreign-owner-file").apply { writeText("keep", Charsets.UTF_8) }
    val outside = root.resolve("journal-outside").apply { check(mkdir()) }
    val symlink = revisions.resolve("e".repeat(64)).toPath()
    Files.createSymbolicLink(symlink, outside.toPath())
    val interrupted = SkillAssetInstaller(
      root,
      InstrumentedSkillAssets(targetFiles),
      AndroidSkillPublicationFileSystem(),
      SkillPublicationFaults { point ->
        if (point == SkillPublicationFaultPoint.PREVIOUS_RENAMED) {
          throw IllegalStateException("injected interruption after previous rename")
        }
      },
    )

    assertThrows(IllegalStateException::class.java) { interrupted.installVerifiedBundle() }
    assertEquals(current.contentHash, currentHash(root))
    assertEquals(current.contentHash, previousHash(root))

    SkillAssetInstaller(root, InstrumentedSkillAssets(currentFiles)).installVerifiedBundle()

    assertEquals(current.contentHash, currentHash(root))
    assertEquals(rollback.contentHash, previousHash(root))
    assertEquals(setOf(current.contentHash, rollback.contentHash), revisionHashes(root))
    assertTrue(foreign.isFile)
    assertTrue(Files.isSymbolicLink(symlink))

    SkillAssetInstaller(root, InstrumentedSkillAssets(rollbackFiles)).installVerifiedBundle()
    assertEquals(rollback.contentHash, currentHash(root))
    assertEquals(current.contentHash, previousHash(root))

    val upgraded = SkillAssetInstaller(root, InstrumentedSkillAssets(targetFiles)).installVerifiedBundle()
    assertEquals(upgraded.contentHash, currentHash(root))
    assertEquals(rollback.contentHash, previousHash(root))
    assertEquals(setOf(upgraded.contentHash, rollback.contentHash), revisionHashes(root))
    assertTrue(foreign.isFile)
    assertTrue(Files.isSymbolicLink(symlink))
    assertTrue(outside.isDirectory)
    root.deleteRecursively()
  }

  @Test
  fun ancestorSwapsFailClosedWithoutOutsideWritesOrNewCurrent() {
    for (ancestor in SwapAncestor.entries) {
      val root = testRoot("fail-${ancestor.name.lowercase()}")
      val stableFiles = bundleFiles("stable-${ancestor.name.lowercase()}")
      val nextFiles = bundleFiles("next-${ancestor.name.lowercase()}")
      SkillAssetInstaller(root, InstrumentedSkillAssets(stableFiles)).installVerifiedBundle()
      val swap = PublicationAncestorSwap(root, ancestor, bundleHash(nextFiles))
      val hooks = AndroidSkillPublicationHooks { event ->
        if (event == ancestor.openedEvent) swap.swap()
      }

      try {
        assertThrows(IllegalStateException::class.java) {
          SkillAssetInstaller(
            root,
            InstrumentedSkillAssets(nextFiles),
            AndroidSkillPublicationFileSystem(hooks),
          ).installVerifiedBundle()
        }
        assertTrue(swap.outside.listFiles().orEmpty().isEmpty())
      } finally {
        swap.restore()
      }

      assertEquals(bundleHash(stableFiles), currentHash(root))
      root.deleteRecursively()
    }
  }

  @Test
  fun ancestorSwapBackPublishesOnlyThroughHeldDirectories() {
    for (ancestor in SwapAncestor.entries) {
      val root = testRoot("restore-${ancestor.name.lowercase()}")
      val stableFiles = bundleFiles("stable-restore-${ancestor.name.lowercase()}")
      val nextFiles = bundleFiles("next-restore-${ancestor.name.lowercase()}")
      SkillAssetInstaller(root, InstrumentedSkillAssets(stableFiles)).installVerifiedBundle()
      val nextHash = bundleHash(nextFiles)
      val swap = PublicationAncestorSwap(root, ancestor, nextHash)
      val hooks = AndroidSkillPublicationHooks { event ->
        when (event) {
          ancestor.openedEvent -> swap.swap()
          AndroidSkillPublicationEvent.FILES_SYNCED -> swap.restore()
          else -> {}
        }
      }

      val installed = try {
        SkillAssetInstaller(
          root,
          InstrumentedSkillAssets(nextFiles),
          AndroidSkillPublicationFileSystem(hooks),
        ).installVerifiedBundle()
      } finally {
        swap.restore()
      }

      assertTrue(installed.changed)
      assertEquals(nextHash, currentHash(root))
      assertTrue(root.resolve("builtin-skills/revisions/$nextHash").isDirectory)
      assertTrue(swap.outside.listFiles().orEmpty().isEmpty())
      root.deleteRecursively()
    }
  }

  private fun testRoot(label: String): File {
    val context = InstrumentationRegistry.getInstrumentation().targetContext
    return context.filesDir.resolve("task15-$label-${UUID.randomUUID()}").apply {
      check(mkdirs())
    }
  }
}

private enum class SwapAncestor(val openedEvent: AndroidSkillPublicationEvent) {
  BUNDLE_ROOT(AndroidSkillPublicationEvent.BUNDLE_ROOT_OPENED),
  REVISIONS(AndroidSkillPublicationEvent.REVISIONS_OPENED),
  INCOMING(AndroidSkillPublicationEvent.INCOMING_OPENED),
}

private class PublicationAncestorSwap(
  root: File,
  ancestor: SwapAncestor,
  contentHash: String,
) {
  private val source = when (ancestor) {
    SwapAncestor.BUNDLE_ROOT -> root.resolve("builtin-skills")
    SwapAncestor.REVISIONS -> root.resolve("builtin-skills/revisions")
    SwapAncestor.INCOMING -> root.resolve("builtin-skills/revisions/.$contentHash.incoming")
  }
  private val held = when (ancestor) {
    SwapAncestor.BUNDLE_ROOT -> root.resolve(".held-builtin-skills")
    SwapAncestor.REVISIONS -> root.resolve("builtin-skills/.held-revisions")
    SwapAncestor.INCOMING -> root.resolve("builtin-skills/revisions/.held-$contentHash-incoming")
  }
  val outside: File = root.resolve("outside-${ancestor.name.lowercase()}").apply {
    check(mkdirs())
  }
  private var swapped = false

  fun swap() {
    if (swapped) return
    check(Files.move(source.toPath(), held.toPath()) == held.toPath())
    Files.createSymbolicLink(source.toPath(), outside.toPath())
    swapped = true
  }

  fun restore() {
    if (!swapped) return
    if (Files.isSymbolicLink(source.toPath())) Files.delete(source.toPath())
    if (!source.exists() && held.exists()) Files.move(held.toPath(), source.toPath())
    swapped = false
  }
}

private class InstrumentedSkillAssets(
  private val files: Map<String, ByteArray>,
) : SkillAssetSource {
  override fun bundleHash(): String = bundleHash(files)

  override fun entries(): List<SkillAssetEntry> =
    files.keys.map { SkillAssetEntry(it, SkillAssetType.FILE) }

  override fun open(relativePath: String): InputStream =
    ByteArrayInputStream(checkNotNull(files[relativePath]))
}

private fun bundleFiles(version: String): Map<String, ByteArray> = mapOf(
  "current" to "{\"schemaVersion\":2,\"active\":{\"generation\":\"$version\"}}"
    .toByteArray(StandardCharsets.UTF_8),
  "generations/$version/skill-bundle.json" to "{\"schemaVersion\":1,\"packages\":[]}"
    .toByteArray(StandardCharsets.UTF_8),
  "generations/$version/skill-bundle.lock" to "{\"schemaVersion\":1,\"packages\":[]}"
    .toByteArray(StandardCharsets.UTF_8),
)

private fun currentHash(root: File): String =
  root.resolve("builtin-skills/current").readText(Charsets.UTF_8).trim()

private fun previousHash(root: File): String =
  root.resolve("builtin-skills/previous").readText(Charsets.UTF_8).trim()

private fun revisionHashes(root: File): Set<String> =
  root.resolve("builtin-skills/revisions").listFiles().orEmpty()
    .filter { file ->
      file.name.matches(Regex("[0-9a-f]{64}")) &&
        Files.isDirectory(file.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS)
    }
    .mapTo(mutableSetOf(), File::getName)

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
