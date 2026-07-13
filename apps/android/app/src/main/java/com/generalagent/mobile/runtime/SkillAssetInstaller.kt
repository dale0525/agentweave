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
    val bundleRootKey = privateRoot.resolve("builtin-skills")

    return processLocks.computeIfAbsent(bundleRootKey) { ReentrantLock() }.withLock {
      if (fileSystem is DirectoryBoundSkillPublication) {
        fileSystem.installDirectoryBound(
          DirectoryBoundSkillPublicationRequest(
            privateRoot = privateRoot,
            expectedHash = expectedHash,
            entries = entries,
            assets = assets,
            faults = faults,
          ),
        )
      } else {
        val bundleRoot = prepareRealDirectory(bundleRootKey, privateRoot)
        fileSystem.withExclusiveLock(bundleRoot.resolve(".install.lock")) {
          installLocked(bundleRoot, expectedHash, entries)
        }
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
    val previousFile = bundleRoot.resolve("previous")
    var currentHash = readPointerHash(currentFile, "current")
    var previousHash = readPointerHash(previousFile, "previous")
    check(currentHash != null || previousHash == null) {
      "Built-in skill previous pointer exists without current"
    }
    val recovered = recoverPointerTransaction(
      bundleRoot,
      currentFile,
      previousFile,
      currentHash,
      previousHash,
      expectedHash,
    )
    currentHash = recovered.current
    previousHash = recovered.previous
    if (currentHash == expectedHash && Files.isDirectory(revision, LinkOption.NOFOLLOW_LINKS)) {
      check(hashPublishedTree(revision) == expectedHash) { "Published built-in skill revision failed verification" }
      check(readPointerHash(currentFile, "current", sync = true) == expectedHash) {
        "Built-in skill current pointer changed during durability recovery"
      }
      cleanupRevisions(revisions, setOfNotNull(expectedHash, previousHash))
      fileSystem.syncDirectory(bundleRoot)
      return InstalledSkillBundle(revision.toFile(), expectedHash, recovered.changed)
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
    if (currentHash != null) {
      writePointerTransaction(
        bundleRoot,
        SkillPointerTransaction(currentHash, previousHash, expectedHash),
      )
      switchPointer(bundleRoot, previousFile, ".previous.incoming", currentHash)
      faults.after(SkillPublicationFaultPoint.PREVIOUS_RENAMED)
    }
    switchCurrent(bundleRoot, currentFile, expectedHash)
    clearPointerTransaction(bundleRoot)
    cleanupRevisions(revisions, setOfNotNull(expectedHash, currentHash))
    return InstalledSkillBundle(revision.toFile(), expectedHash, true)
  }

  private data class PointerRecovery(
    val current: String?,
    val previous: String?,
    val changed: Boolean,
  )

  private fun recoverPointerTransaction(
    bundleRoot: Path,
    currentFile: Path,
    previousFile: Path,
    currentHash: String?,
    previousHash: String?,
    expectedHash: String,
  ): PointerRecovery {
    Files.deleteIfExists(bundleRoot.resolve(TRANSACTION_INCOMING))
    val transactionFile = bundleRoot.resolve(TRANSACTION)
    if (!Files.exists(transactionFile, LinkOption.NOFOLLOW_LINKS)) {
      return PointerRecovery(currentHash, previousHash, false)
    }
    check(Files.isRegularFile(transactionFile, LinkOption.NOFOLLOW_LINKS)) {
      "Built-in skill pointer transaction is not a regular file"
    }
    val transaction = SkillPointerTransaction.decode(fileSystem.readVerifiedFile(transactionFile))
    return when (
      skillPointerRecoveryAction(transaction, currentHash, previousHash, expectedHash)
    ) {
      SkillPointerRecoveryAction.ABORT -> {
        restorePreviousPointer(bundleRoot, previousFile, previousHash, transaction.oldPrevious)
        clearPointerTransaction(bundleRoot)
        PointerRecovery(transaction.oldCurrent, transaction.oldPrevious, false)
      }
      SkillPointerRecoveryAction.FINALIZE -> {
        clearPointerTransaction(bundleRoot)
        PointerRecovery(transaction.target, transaction.oldCurrent, false)
      }
      SkillPointerRecoveryAction.RESUME -> {
        if (previousHash != transaction.oldCurrent) {
          switchPointer(bundleRoot, previousFile, ".previous.incoming", transaction.oldCurrent)
          faults.after(SkillPublicationFaultPoint.PREVIOUS_RENAMED)
        }
        switchCurrent(bundleRoot, currentFile, transaction.target)
        clearPointerTransaction(bundleRoot)
        PointerRecovery(transaction.target, transaction.oldCurrent, true)
      }
    }
  }

  private fun writePointerTransaction(bundleRoot: Path, transaction: SkillPointerTransaction) {
    val incoming = bundleRoot.resolve(TRANSACTION_INCOMING)
    val target = bundleRoot.resolve(TRANSACTION)
    Files.deleteIfExists(incoming)
    try {
      fileSystem.writeNewFile(incoming, transaction.encode())
      fileSystem.atomicMove(incoming, target, replace = true)
      fileSystem.syncDirectory(bundleRoot)
    } catch (error: Exception) {
      Files.deleteIfExists(incoming)
      throw IllegalStateException("Failed to prepare built-in skill pointer transaction", error)
    }
  }

  private fun clearPointerTransaction(bundleRoot: Path) {
    Files.deleteIfExists(bundleRoot.resolve(TRANSACTION_INCOMING))
    if (Files.deleteIfExists(bundleRoot.resolve(TRANSACTION))) {
      fileSystem.syncDirectory(bundleRoot)
    }
  }

  private fun restorePreviousPointer(
    bundleRoot: Path,
    previousFile: Path,
    observed: String?,
    restored: String?,
  ) {
    if (observed == restored) return
    if (restored == null) {
      Files.deleteIfExists(previousFile)
      fileSystem.syncDirectory(bundleRoot)
    } else {
      switchPointer(bundleRoot, previousFile, ".previous.incoming", restored)
    }
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
    val incomingName = ".current.incoming"
    val incoming = bundleRoot.resolve(incomingName)
    Files.deleteIfExists(incoming)
    try {
      writePointer(incoming, expectedHash)
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

  private fun switchPointer(bundleRoot: Path, target: Path, incomingName: String, hash: String) {
    val incoming = bundleRoot.resolve(incomingName)
    Files.deleteIfExists(incoming)
    try {
      writePointer(incoming, hash)
      fileSystem.atomicMove(incoming, target, replace = true)
      fileSystem.syncDirectory(bundleRoot)
    } catch (error: Exception) {
      Files.deleteIfExists(incoming)
      throw IllegalStateException("Failed to switch built-in skill rollback pointer", error)
    }
  }

  private fun writePointer(path: Path, hash: String) {
    fileSystem.writeNewFile(path, hash.toByteArray(StandardCharsets.UTF_8))
  }

  private fun cleanupRevisions(revisions: Path, retained: Set<String>) {
    var changed = false
    Files.newDirectoryStream(revisions).use { entries ->
      for (entry in entries) {
        val name = entry.fileName.toString()
        if (name !in retained && HASH_PATTERN.matches(name) && Files.isDirectory(entry, LinkOption.NOFOLLOW_LINKS)) {
          deleteTreeNoFollow(entry)
          changed = true
        }
      }
    }
    if (changed) fileSystem.syncDirectory(revisions)
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

  private fun readPointerHash(pointer: Path, label: String, sync: Boolean = false): String? {
    if (!Files.exists(pointer, LinkOption.NOFOLLOW_LINKS)) return null
    check(Files.isRegularFile(pointer, LinkOption.NOFOLLOW_LINKS)) {
      "Built-in skill $label pointer is not a regular file"
    }
    val hash = fileSystem.readVerifiedFile(pointer, sync).toString(StandardCharsets.UTF_8).trim()
    check(HASH_PATTERN.matches(hash)) { "Built-in skill $label pointer hash is invalid" }
    return hash
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
    private const val TRANSACTION = ".pointer-transaction"
    private const val TRANSACTION_INCOMING = ".pointer-transaction.incoming"
    private val HASH_PATTERN = Regex("[0-9a-f]{64}")
    private val processLocks = ConcurrentHashMap<Path, ReentrantLock>()
  }
}
