package com.generalagent.mobile.runtime

import java.io.ByteArrayInputStream
import java.io.File
import java.io.InputStream
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.security.MessageDigest
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
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
    val installer = testInstaller(temporaryFolder.root, assets)

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
    val first = testInstaller(temporaryFolder.root, FakeSkillAssets(firstFiles))
      .installVerifiedBundle()
    val second = testInstaller(temporaryFolder.root, FakeSkillAssets(secondFiles))
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
      testInstaller(temporaryFolder.root, FakeSkillAssets(files)).installVerifiedBundle()
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
        testInstaller(temporaryFolder.root, assets).installVerifiedBundle()
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
      testInstaller(temporaryFolder.root, FakeSkillAssets(validBundleFiles("v1")))
        .installVerifiedBundle()
    }

    assertEquals("unchanged", outside.readText(Charsets.UTF_8))
  }

  @Test
  fun rejectsHardLinkedInstallerLock() {
    val bundleRoot = temporaryFolder.root.resolve("builtin-skills")
    assertTrue(bundleRoot.mkdirs())
    val outside = temporaryFolder.newFile("outside-hardlink-lock").toPath()
    Files.createLink(bundleRoot.resolve(".install.lock").toPath(), outside)

    assertThrows(IllegalStateException::class.java) {
      testInstaller(temporaryFolder.root, FakeSkillAssets(validBundleFiles("v1")))
        .installVerifiedBundle()
    }
  }

  @Test
  fun rejectsBundleWhoseDeclaredHashDoesNotMatchContent() {
    val files = validBundleFiles("v1")
    val assets = FakeSkillAssets(files, declaredHash = "0".repeat(64))

    assertThrows(IllegalStateException::class.java) {
      testInstaller(temporaryFolder.root, assets).installVerifiedBundle()
    }

    assertFalse(temporaryFolder.root.resolve("builtin-skills/current").exists())
  }

  @Test
  fun failedCopyPreservesLastKnownGoodCurrentRevision() {
    val goodFiles = validBundleFiles("v1")
    val first = testInstaller(temporaryFolder.root, FakeSkillAssets(goodFiles))
      .installVerifiedBundle()
    val badFiles = validBundleFiles("v2") + ("packages/fail" to byteArrayOf(9))
    val failing = FakeSkillAssets(badFiles, failOnOpen = "packages/fail")

    assertThrows(IllegalStateException::class.java) {
      testInstaller(temporaryFolder.root, failing).installVerifiedBundle()
    }

    assertEquals(first.contentHash, currentHash(temporaryFolder.root))
    assertTrue(first.root.isDirectory)
    assertFalse(
      temporaryFolder.root.resolve("builtin-skills/revisions/.${bundleHash(badFiles)}.incoming").exists(),
    )
  }

  @Test
  fun concurrentInstallersForSameCanonicalRootWaitAndBothComplete() {
    val entered = CountDownLatch(1)
    val release = CountDownLatch(1)
    val files = validBundleFiles("concurrent")
    val blockingAssets = FakeSkillAssets(
      files,
      onOpen = { path ->
        if (path == "current") {
          entered.countDown()
          check(release.await(5, TimeUnit.SECONDS)) { "timed out waiting to release first installer" }
        }
      },
    )
    val results = mutableListOf<Result<InstalledSkillBundle>>()
    val resultsLock = Any()
    val first = Thread {
      val result = runCatching {
        testInstaller(temporaryFolder.root, blockingAssets).installVerifiedBundle()
      }
      synchronized(resultsLock) { results += result }
    }
    val second = Thread {
      val result = runCatching {
        testInstaller(temporaryFolder.root, FakeSkillAssets(files)).installVerifiedBundle()
      }
      synchronized(resultsLock) { results += result }
    }

    first.start()
    assertTrue(entered.await(5, TimeUnit.SECONDS))
    second.start()
    Thread.sleep(100)
    assertTrue("second installer must wait for the first", second.isAlive)
    release.countDown()
    first.join(5_000)
    second.join(5_000)

    assertEquals(2, results.size)
    assertTrue(results.all(Result<InstalledSkillBundle>::isSuccess))
    assertEquals(1, results.count { it.getOrThrow().changed })
    assertEquals(bundleHash(files), currentHash(temporaryFolder.root))
    assertEquals(1, temporaryFolder.root.resolve("builtin-skills/revisions").listFiles()!!.count { it.isDirectory })
  }

  @Test
  fun durabilityFaultsNeverPublishCurrentBeforeItsRevisionIsDurable() {
    for (point in SkillPublicationFaultPoint.entries) {
      val filesDir = temporaryFolder.newFolder("fault-${point.name.lowercase()}")
      val firstFiles = validBundleFiles("stable")
      val first = SkillAssetInstaller(
        filesDir,
        FakeSkillAssets(firstFiles),
        JvmSkillPublicationFileSystem(),
      ).installVerifiedBundle()
      val nextFiles = validBundleFiles("next-${point.name.lowercase()}")
      val failing = SkillAssetInstaller(
        filesDir,
        FakeSkillAssets(nextFiles),
        JvmSkillPublicationFileSystem(),
        SkillPublicationFaults { observed ->
          if (observed == point) throw IllegalStateException("injected durability fault: $point")
        },
      )

      assertThrows(IllegalStateException::class.java) {
        failing.installVerifiedBundle()
      }

      val current = currentHash(filesDir)
      val expected = if (point >= SkillPublicationFaultPoint.CURRENT_RENAMED) {
        bundleHash(nextFiles)
      } else {
        first.contentHash
      }
      assertEquals("fault point $point", expected, current)
      assertPublishedRevisionMatchesCurrent(filesDir, current)
    }
  }

  @Test
  fun retryAfterRevisionRenameFaultResyncsRevisionBeforeCurrentSwitch() {
    val stableFiles = validBundleFiles("stable-retry")
    testInstaller(temporaryFolder.root, FakeSkillAssets(stableFiles)).installVerifiedBundle()
    val nextFiles = validBundleFiles("next-retry")
    val failedPublication = SkillAssetInstaller(
      temporaryFolder.root,
      FakeSkillAssets(nextFiles),
      JvmSkillPublicationFileSystem(),
      SkillPublicationFaults { point ->
        if (point == SkillPublicationFaultPoint.REVISION_RENAMED) {
          throw IllegalStateException("injected revision parent sync failure")
        }
      },
    )
    assertThrows(IllegalStateException::class.java) {
      failedPublication.installVerifiedBundle()
    }

    val retryFileSystem = RequireRevisionSyncBeforeCurrentMove()
    SkillAssetInstaller(
      temporaryFolder.root,
      FakeSkillAssets(nextFiles),
      retryFileSystem,
    ).installVerifiedBundle()

    assertTrue(retryFileSystem.revisionFileSynced)
    assertTrue(retryFileSystem.revisionDirectorySynced)
    assertTrue(retryFileSystem.revisionsParentSynced)
    assertEquals(bundleHash(nextFiles), currentHash(temporaryFolder.root))
  }

  @Test
  fun rejectsHardLinkedPublishedRevisionFile() {
    val files = validBundleFiles("hardlink")
    val installed = SkillAssetInstaller(
      temporaryFolder.root,
      FakeSkillAssets(files),
      JvmSkillPublicationFileSystem(),
    ).installVerifiedBundle()
    val manifest = installed.root.resolve("generations/hardlink/skill-bundle.json").toPath()
    val outside = temporaryFolder.newFile("outside-manifest").toPath()
    Files.delete(outside)
    Files.createLink(outside, manifest)

    assertThrows(IllegalStateException::class.java) {
      SkillAssetInstaller(
        temporaryFolder.root,
        FakeSkillAssets(files),
        JvmSkillPublicationFileSystem(),
      ).installVerifiedBundle()
    }
  }

  @Test
  fun rejectsRevisionPathSwapAfterVerifiedHandleOpens() {
    val files = validBundleFiles("swap")
    val installed = SkillAssetInstaller(
      temporaryFolder.root,
      FakeSkillAssets(files),
      JvmSkillPublicationFileSystem(),
    ).installVerifiedBundle()
    val manifest = installed.root.resolve("generations/swap/skill-bundle.json").toPath()
    val original = manifest.resolveSibling("skill-bundle.original")
    val outside = temporaryFolder.newFile("swapped-manifest").toPath()
    Files.write(outside, Files.readAllBytes(manifest))
    var swapped = false
    val swappingFileSystem = JvmSkillPublicationFileSystem(afterVerifiedOpen = { opened ->
      if (!swapped && opened == manifest) {
        swapped = true
        Files.move(manifest, original)
        Files.createSymbolicLink(manifest, outside)
      }
    })

    assertThrows(IllegalStateException::class.java) {
      SkillAssetInstaller(
        temporaryFolder.root,
        FakeSkillAssets(files),
        swappingFileSystem,
      ).installVerifiedBundle()
    }
    assertTrue(swapped)
  }
}

private class RequireRevisionSyncBeforeCurrentMove : JvmSkillPublicationFileSystem() {
  var revisionFileSynced = false
  var revisionDirectorySynced = false
  var revisionsParentSynced = false

  override fun readVerifiedFile(path: java.nio.file.Path, sync: Boolean): ByteArray {
    if (sync && path.parent?.parent?.fileName?.toString() == "generations") {
      revisionFileSynced = true
    }
    return super.readVerifiedFile(path, sync)
  }

  override fun syncDirectory(path: java.nio.file.Path) {
    when {
      path.fileName.toString() == "revisions" -> revisionsParentSynced = true
      path.parent?.fileName?.toString()?.matches(Regex("[0-9a-f]{64}")) == true -> {
        revisionDirectorySynced = true
      }
    }
    super.syncDirectory(path)
  }

  override fun atomicMove(source: java.nio.file.Path, target: java.nio.file.Path, replace: Boolean) {
    if (target.fileName.toString() == "current") {
      check(revisionFileSynced && revisionDirectorySynced && revisionsParentSynced) {
        "current switched before the retained revision was resynced"
      }
    }
    super.atomicMove(source, target, replace)
  }
}

private class FakeSkillAssets(
  private val files: Map<String, ByteArray>,
  private val declaredHash: String = bundleHash(files),
  private val extraEntries: List<SkillAssetEntry> = emptyList(),
  private val failOnOpen: String? = null,
  private val onOpen: (String) -> Unit = {},
) : SkillAssetSource {
  override fun bundleHash(): String = declaredHash

  override fun entries(): List<SkillAssetEntry> =
    files.keys.map { SkillAssetEntry(it, SkillAssetType.FILE) } + extraEntries

  override fun open(relativePath: String): InputStream {
    check(relativePath != failOnOpen) { "injected asset read failure" }
    onOpen(relativePath)
    return ByteArrayInputStream(checkNotNull(files[relativePath]))
  }
}

private fun testInstaller(
  filesDir: File,
  assets: SkillAssetSource,
): SkillAssetInstaller = SkillAssetInstaller(
  filesDir,
  assets,
  JvmSkillPublicationFileSystem(),
)

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

private fun assertPublishedRevisionMatchesCurrent(filesDir: File, hash: String) {
  val revision = filesDir.resolve("builtin-skills/revisions/$hash")
  assertTrue("current must reference a published revision", revision.isDirectory)
  assertTrue(revision.resolve("current").isFile)
  assertTrue(revision.walkTopDown().any { it.name == "skill-bundle.json" })
  assertTrue(revision.walkTopDown().any { it.name == "skill-bundle.lock" })
}

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
