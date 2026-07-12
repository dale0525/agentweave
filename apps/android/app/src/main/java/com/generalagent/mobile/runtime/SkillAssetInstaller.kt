package com.generalagent.mobile.runtime

import android.content.res.AssetManager
import java.io.File
import java.io.InputStream
import java.nio.charset.StandardCharsets
import java.nio.file.FileVisitResult
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.Path
import java.nio.file.SimpleFileVisitor
import java.nio.file.attribute.BasicFileAttributes
import java.security.MessageDigest
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

enum class SkillAssetType {
  FILE,
  DIRECTORY,
  SYMLINK,
  SPECIAL,
}

data class SkillAssetEntry(val relativePath: String, val type: SkillAssetType)

interface SkillAssetSource {
  fun bundleHash(): String

  fun entries(): List<SkillAssetEntry>

  fun open(relativePath: String): InputStream
}

data class InstalledSkillBundle(
  val root: File,
  val contentHash: String,
  val changed: Boolean,
)

class AndroidSkillAssetSource(
  private val assets: AssetManager,
  private val assetRoot: String = "builtin-skills",
) : SkillAssetSource {
  override fun bundleHash(): String =
    assets.open("$assetRoot/bundle.sha256").bufferedReader(StandardCharsets.UTF_8).use { it.readText().trim() }

  override fun entries(): List<SkillAssetEntry> {
    val entries = mutableListOf<SkillAssetEntry>()
    collect("$assetRoot/bundle", "", entries)
    return entries
  }

  override fun open(relativePath: String): InputStream =
    assets.open("$assetRoot/bundle/$relativePath", AssetManager.ACCESS_STREAMING)

  private fun collect(assetPath: String, relativePath: String, output: MutableList<SkillAssetEntry>) {
    val children = assets.list(assetPath)?.sorted().orEmpty()
    if (children.isEmpty()) {
      require(relativePath.isNotEmpty()) { "Built-in skill asset bundle is empty" }
      output += SkillAssetEntry(relativePath, SkillAssetType.FILE)
      return
    }
    if (relativePath.isNotEmpty()) {
      output += SkillAssetEntry(relativePath, SkillAssetType.DIRECTORY)
    }
    for (child in children) {
      collect("$assetPath/$child", if (relativePath.isEmpty()) child else "$relativePath/$child", output)
    }
  }
}

class SkillAssetInstaller internal constructor(
  private val filesDir: File,
  private val assets: SkillAssetSource,
  private val fileSystem: SkillPublicationFileSystem,
  private val faults: SkillPublicationFaults = SkillPublicationFaults.NONE,
) {
  constructor(
    filesDir: File,
    assets: SkillAssetSource,
  ) : this(filesDir, assets, AndroidSkillPublicationFileSystem(), SkillPublicationFaults.NONE)

  fun installVerifiedBundle(): InstalledSkillBundle {
    val expectedHash = assets.bundleHash().trim()
    require(HASH_PATTERN.matches(expectedHash)) { "Built-in skill bundle hash is invalid" }
    val entries = validateEntries(assets.entries())
    val privateRoot = preparePrivateRoot(filesDir.toPath())
    val bundleRoot = prepareRealDirectory(privateRoot.resolve("builtin-skills"), privateRoot)
    val lockPath = bundleRoot.resolve(".install.lock")

    return processLocks.computeIfAbsent(bundleRoot) { ReentrantLock() }.withLock {
      fileSystem.withExclusiveLock(lockPath) {
        installLocked(bundleRoot, expectedHash, entries)
      }
    }
  }

  private fun installLocked(
    bundleRoot: Path,
    expectedHash: String,
    entries: List<SkillAssetEntry>,
  ): InstalledSkillBundle {
    val revisions = prepareRealDirectory(bundleRoot.resolve("revisions"), bundleRoot)
    val revision = containedPath(revisions, expectedHash)
    val currentFile = bundleRoot.resolve("current")
    val currentHash = readCurrentHash(currentFile)
    if (currentHash == expectedHash && Files.isDirectory(revision, LinkOption.NOFOLLOW_LINKS)) {
      check(hashPublishedTree(revision) == expectedHash) { "Published built-in skill revision failed verification" }
      return InstalledSkillBundle(revision.toFile(), expectedHash, false)
    }

    if (Files.exists(revision, LinkOption.NOFOLLOW_LINKS)) {
      check(Files.isDirectory(revision, LinkOption.NOFOLLOW_LINKS)) {
        "Built-in skill revision path is not a real directory"
      }
      check(hashPublishedTree(revision, syncFiles = true) == expectedHash) {
        "Published built-in skill revision failed verification"
      }
      syncDirectoriesBottomUp(revision)
      fileSystem.syncDirectory(revisions)
    } else {
      publishRevision(revisions, revision, expectedHash, entries)
    }
    switchCurrent(bundleRoot, currentFile, expectedHash)
    return InstalledSkillBundle(revision.toFile(), expectedHash, true)
  }

  private fun publishRevision(
    revisions: Path,
    revision: Path,
    expectedHash: String,
    entries: List<SkillAssetEntry>,
  ) {
    val incoming = containedPath(revisions, ".$expectedHash.incoming")
    deleteTreeNoFollow(incoming)
    Files.createDirectory(incoming)
    try {
      val digest = MessageDigest.getInstance("SHA-256")
      for (entry in entries) {
        val target = containedPath(incoming, entry.relativePath)
        when (entry.type) {
          SkillAssetType.DIRECTORY -> Files.createDirectories(target)
          SkillAssetType.FILE -> {
            Files.createDirectories(checkNotNull(target.parent))
            val bytes = assets.open(entry.relativePath).use { it.readBytes() }
            updateDigest(digest, entry.relativePath, bytes)
            fileSystem.writeNewFile(target, bytes)
          }
          SkillAssetType.SYMLINK, SkillAssetType.SPECIAL -> error("unreachable asset type")
        }
      }
      faults.after(SkillPublicationFaultPoint.FILES_SYNCED)
      check(digest.digest().toHex() == expectedHash) { "Built-in skill bundle content hash mismatch" }
      check(hashPublishedTree(incoming) == expectedHash) {
        "Incoming built-in skill revision failed handle verification"
      }
      syncDirectoriesBottomUp(incoming)
      faults.after(SkillPublicationFaultPoint.DIRECTORIES_SYNCED)
      fileSystem.atomicMove(incoming, revision, replace = false)
      faults.after(SkillPublicationFaultPoint.REVISION_RENAMED)
      fileSystem.syncDirectory(revisions)
      faults.after(SkillPublicationFaultPoint.REVISIONS_SYNCED)
    } catch (error: Exception) {
      deleteTreeNoFollow(incoming)
      if (error is IllegalStateException) throw error
      throw IllegalStateException("Failed to publish built-in skill revision", error)
    }
  }

  private fun switchCurrent(bundleRoot: Path, currentFile: Path, expectedHash: String) {
    val incoming = bundleRoot.resolve(".current.incoming")
    Files.deleteIfExists(incoming)
    try {
      fileSystem.writeNewFile(incoming, expectedHash.toByteArray(StandardCharsets.UTF_8))
      faults.after(SkillPublicationFaultPoint.CURRENT_TEMP_SYNCED)
      fileSystem.atomicMove(incoming, currentFile, replace = true)
      faults.after(SkillPublicationFaultPoint.CURRENT_RENAMED)
      fileSystem.syncDirectory(bundleRoot)
      faults.after(SkillPublicationFaultPoint.BUNDLE_ROOT_SYNCED)
    } catch (error: Exception) {
      Files.deleteIfExists(incoming)
      throw IllegalStateException("Failed to switch built-in skill revision", error)
    }
  }

  private fun validateEntries(unvalidated: List<SkillAssetEntry>): List<SkillAssetEntry> {
    require(unvalidated.isNotEmpty()) { "Built-in skill asset bundle is empty" }
    val seen = mutableSetOf<String>()
    val entries = unvalidated.sortedWith(compareBy<SkillAssetEntry>({ it.relativePath }, { it.type.name }))
    for (entry in entries) {
      validateRelativePath(entry.relativePath)
      require(seen.add(entry.relativePath)) { "Duplicate built-in skill asset entry: ${entry.relativePath}" }
      require(entry.type == SkillAssetType.FILE || entry.type == SkillAssetType.DIRECTORY) {
        "Built-in skill assets must contain only regular files and directories"
      }
    }
    val files = entries.filter { it.type == SkillAssetType.FILE }.mapTo(mutableSetOf()) { it.relativePath }
    require("current" in files) { "Built-in skill bundle current metadata is missing" }
    require(files.any { it.endsWith("/skill-bundle.json") }) {
      "Built-in skill bundle manifest is missing"
    }
    require(files.any { it.endsWith("/skill-bundle.lock") }) {
      "Built-in skill bundle lock is missing"
    }
    return entries
  }

  private fun validateRelativePath(relativePath: String) {
    require(relativePath.isNotEmpty() && '\u0000' !in relativePath && '\\' !in relativePath) {
      "Invalid built-in skill asset path"
    }
    val path = Path.of(relativePath)
    require(!path.isAbsolute && path.nameCount > 0) { "Built-in skill asset path must be relative" }
    require(path.none { it.toString() == "." || it.toString() == ".." }) {
      "Built-in skill asset path traversal is not allowed"
    }
    require(path.normalize() == path) { "Built-in skill asset path traversal is not allowed" }
  }

  private fun hashPublishedTree(root: Path, syncFiles: Boolean = false): String {
    val files = mutableListOf<Path>()
    val directories = mutableMapOf<Path, String>()
    Files.walkFileTree(root, object : SimpleFileVisitor<Path>() {
      override fun preVisitDirectory(dir: Path, attrs: BasicFileAttributes): FileVisitResult {
        check(!attrs.isSymbolicLink) { "Published built-in skill revision contains a symlink" }
        directories[dir] = fileSystem.directoryIdentity(dir)
        return FileVisitResult.CONTINUE
      }

      override fun visitFile(file: Path, attrs: BasicFileAttributes): FileVisitResult {
        check(attrs.isRegularFile && !attrs.isSymbolicLink) {
          "Published built-in skill revision contains a special file"
        }
        files.add(file)
        return FileVisitResult.CONTINUE
      }
    })
    val digest = MessageDigest.getInstance("SHA-256")
    for (file in files.sortedBy { root.relativize(it).toString() }) {
      val relativePath = root.relativize(file).joinToString("/")
      updateDigest(digest, relativePath, fileSystem.readVerifiedFile(file, syncFiles))
    }
    for ((directory, identity) in directories) {
      check(fileSystem.directoryIdentity(directory) == identity) {
        "Published built-in skill directory identity changed during verification"
      }
    }
    return digest.digest().toHex()
  }

  private fun readCurrentHash(currentFile: Path): String? {
    if (!Files.exists(currentFile, LinkOption.NOFOLLOW_LINKS)) return null
    check(Files.isRegularFile(currentFile, LinkOption.NOFOLLOW_LINKS)) {
      "Built-in skill current pointer is not a regular file"
    }
    return fileSystem.readVerifiedFile(currentFile).toString(StandardCharsets.UTF_8).trim()
  }

  private fun preparePrivateRoot(path: Path): Path {
    if (Files.exists(path, LinkOption.NOFOLLOW_LINKS)) {
      require(Files.isDirectory(path, LinkOption.NOFOLLOW_LINKS) && !Files.isSymbolicLink(path)) {
        "App-private files root must be a real directory"
      }
    } else {
      Files.createDirectories(path)
    }
    return path.toRealPath(LinkOption.NOFOLLOW_LINKS)
  }

  private fun prepareRealDirectory(path: Path, allowedRoot: Path): Path {
    val contained = containedPath(allowedRoot, allowedRoot.relativize(path).toString())
    if (Files.exists(contained, LinkOption.NOFOLLOW_LINKS)) {
      check(Files.isDirectory(contained, LinkOption.NOFOLLOW_LINKS) && !Files.isSymbolicLink(contained)) {
        "Built-in skill storage path must be a real directory"
      }
    } else {
      Files.createDirectory(contained)
    }
    val real = contained.toRealPath(LinkOption.NOFOLLOW_LINKS)
    check(real.startsWith(allowedRoot)) { "Built-in skill storage escaped app-private files" }
    return real
  }

  private fun containedPath(root: Path, relativePath: String): Path {
    val target = root.resolve(relativePath).normalize()
    require(target.startsWith(root)) { "Built-in skill asset path escaped app-private files" }
    return target
  }

  private fun syncDirectoriesBottomUp(root: Path) {
    val directories = mutableListOf<Path>()
    Files.walkFileTree(root, object : SimpleFileVisitor<Path>() {
      override fun preVisitDirectory(dir: Path, attrs: BasicFileAttributes): FileVisitResult {
        check(attrs.isDirectory && !attrs.isSymbolicLink) {
          "Built-in skill revision directory changed during publication"
        }
        directories.add(dir)
        return FileVisitResult.CONTINUE
      }
    })
    for (directory in directories.sortedByDescending(Path::getNameCount)) {
      fileSystem.syncDirectory(directory)
    }
  }

  private fun deleteTreeNoFollow(root: Path) {
    if (!Files.exists(root, LinkOption.NOFOLLOW_LINKS)) return
    Files.walkFileTree(root, object : SimpleFileVisitor<Path>() {
      override fun visitFile(file: Path, attrs: BasicFileAttributes): FileVisitResult {
        Files.delete(file)
        return FileVisitResult.CONTINUE
      }

      override fun postVisitDirectory(dir: Path, error: java.io.IOException?): FileVisitResult {
        if (error != null) throw error
        Files.delete(dir)
        return FileVisitResult.CONTINUE
      }
    })
  }

  private fun updateDigest(digest: MessageDigest, relativePath: String, bytes: ByteArray) {
    digest.update(relativePath.toByteArray(StandardCharsets.UTF_8))
    digest.update(0)
    digest.update(bytes.size.toString().toByteArray(StandardCharsets.US_ASCII))
    digest.update(0)
    digest.update(bytes)
  }

  private fun ByteArray.toHex(): String = joinToString("") { byte -> "%02x".format(byte) }

  companion object {
    private val HASH_PATTERN = Regex("[0-9a-f]{64}")
    private val processLocks = ConcurrentHashMap<Path, ReentrantLock>()
  }
}
