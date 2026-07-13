package com.generalagent.mobile.runtime

import android.os.ParcelFileDescriptor
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import android.system.StructStat
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.FileOutputStream
import java.nio.charset.StandardCharsets
import java.nio.file.Path
import java.security.MessageDigest

internal enum class AndroidSkillPublicationEvent {
  ROOT_OPENED,
  BUNDLE_ROOT_OPENED,
  REVISIONS_OPENED,
  INCOMING_OPENED,
  FILES_SYNCED,
  REVISION_RENAMED,
  CURRENT_RENAMED,
}

internal fun interface AndroidSkillPublicationHooks {
  fun after(event: AndroidSkillPublicationEvent)

  companion object {
    val NONE = AndroidSkillPublicationHooks {}
  }
}

internal class AndroidDirectoryBoundSkillPublisher(
  private val request: DirectoryBoundSkillPublicationRequest,
  private val hooks: AndroidSkillPublicationHooks,
) {
  fun install(): InstalledSkillBundle =
    AndroidPublicationDirectory.openRoot(request.privateRoot).use { privateRoot ->
      hooks.after(AndroidSkillPublicationEvent.ROOT_OPENED)
      privateRoot.openOrCreateDirectory(BUNDLE_ROOT).use { bundleRoot ->
        hooks.after(AndroidSkillPublicationEvent.BUNDLE_ROOT_OPENED)
        bundleRoot.withExclusiveLock(LOCK_FILE) {
          bundleRoot.openOrCreateDirectory(REVISIONS).use { revisions ->
            hooks.after(AndroidSkillPublicationEvent.REVISIONS_OPENED)
            installLocked(privateRoot, bundleRoot, revisions)
          }
        }
      }
    }

  private fun installLocked(
    privateRoot: AndroidPublicationDirectory,
    bundleRoot: AndroidPublicationDirectory,
    revisions: AndroidPublicationDirectory,
  ): InstalledSkillBundle {
    val currentHash = readPointerHash(bundleRoot, CURRENT)
    val previousHash = readPointerHash(bundleRoot, PREVIOUS)
    check(currentHash != null || previousHash == null) {
      "Built-in skill previous pointer exists without current"
    }
    if (currentHash == request.expectedHash && revisions.entryKind(request.expectedHash) == EntryKind.DIRECTORY) {
      revisions.openDirectory(request.expectedHash).use { revision ->
        check(hashTree(revision, syncFiles = false, syncDirectories = false) == request.expectedHash) {
          "Published built-in skill revision failed verification"
        }
        check(readPointerHash(bundleRoot, CURRENT, sync = true) == request.expectedHash) {
          "Built-in skill current pointer changed during durability recovery"
        }
        cleanupRevisions(revisions, setOfNotNull(request.expectedHash, previousHash))
        bundleRoot.sync()
        privateRoot.sync()
        verifyRootChain(privateRoot, bundleRoot, revisions, revision)
      }
      return installed(changed = false)
    }

    var retainedRevision: AndroidPublicationDirectory? = null
    var retainedTree: HeldDirectoryTree? = null
    try {
      when (revisions.entryKind(request.expectedHash)) {
        null -> {
          retainedTree = publishRevision(revisions)
          retainedRevision = retainedTree.root
        }
        EntryKind.DIRECTORY -> {
          retainedRevision = revisions.openDirectory(request.expectedHash)
          check(hashTree(retainedRevision, syncFiles = true, syncDirectories = true) == request.expectedHash) {
            "Published built-in skill revision failed verification"
          }
          revisions.sync()
        }
        else -> error("Built-in skill revision path is not a real directory")
      }
      verifyRootChain(privateRoot, bundleRoot, revisions, retainedRevision)
      if (currentHash != null) switchPointer(bundleRoot, PREVIOUS, PREVIOUS_INCOMING, currentHash)
      switchCurrent(bundleRoot)
      privateRoot.sync()
      verifyRootChain(privateRoot, bundleRoot, revisions, retainedRevision)
      cleanupRevisions(revisions, setOfNotNull(request.expectedHash, currentHash))
      return installed(changed = true)
    } finally {
      if (retainedTree != null) {
        retainedTree.close()
      } else {
        retainedRevision?.close()
      }
    }
  }

  private fun publishRevision(revisions: AndroidPublicationDirectory): HeldDirectoryTree {
    val incomingName = ".${request.expectedHash}.incoming"
    revisions.deleteTree(incomingName)
    val tree = HeldDirectoryTree(revisions.createDirectory(incomingName))
    try {
      hooks.after(AndroidSkillPublicationEvent.INCOMING_OPENED)
      val digest = MessageDigest.getInstance("SHA-256")
      for (entry in request.entries) {
        when (entry.type) {
          SkillAssetType.DIRECTORY -> tree.ensureDirectory(entry.relativePath)
          SkillAssetType.FILE -> {
            val parentPath = entry.relativePath.substringBeforeLast('/', "")
            val name = entry.relativePath.substringAfterLast('/')
            val parent = tree.ensureDirectory(parentPath)
            val bytes = request.assets.open(entry.relativePath).use { it.readBytes() }
            updateDigest(digest, entry.relativePath, bytes)
            parent.writeNewFile(name, bytes)
          }
          SkillAssetType.SYMLINK, SkillAssetType.SPECIAL -> error("unreachable asset type")
        }
      }
      hooks.after(AndroidSkillPublicationEvent.FILES_SYNCED)
      request.faults.after(SkillPublicationFaultPoint.FILES_SYNCED)
      check(digest.digest().toHex() == request.expectedHash) {
        "Built-in skill bundle content hash mismatch"
      }
      check(hashTree(tree.root, syncFiles = false, syncDirectories = true) == request.expectedHash) {
        "Incoming built-in skill revision failed handle verification"
      }
      request.faults.after(SkillPublicationFaultPoint.DIRECTORIES_SYNCED)
      revisions.renameVerifiedDirectory(incomingName, tree.root.identity, request.expectedHash)
      hooks.after(AndroidSkillPublicationEvent.REVISION_RENAMED)
      request.faults.after(SkillPublicationFaultPoint.REVISION_RENAMED)
      revisions.sync()
      request.faults.after(SkillPublicationFaultPoint.REVISIONS_SYNCED)
      return tree
    } catch (error: Exception) {
      tree.close()
      revisions.deleteTree(incomingName)
      if (error is IllegalStateException) throw error
      throw IllegalStateException("Failed to publish built-in skill revision", error)
    }
  }

  private fun switchCurrent(bundleRoot: AndroidPublicationDirectory) {
    bundleRoot.deleteTree(CURRENT_INCOMING)
    try {
      val identity = bundleRoot.writeNewFile(CURRENT_INCOMING, pointerBytes(request.expectedHash))
      request.faults.after(SkillPublicationFaultPoint.CURRENT_TEMP_SYNCED)
      bundleRoot.renameVerifiedFile(CURRENT_INCOMING, identity, CURRENT)
      hooks.after(AndroidSkillPublicationEvent.CURRENT_RENAMED)
      request.faults.after(SkillPublicationFaultPoint.CURRENT_RENAMED)
      bundleRoot.sync()
      request.faults.after(SkillPublicationFaultPoint.BUNDLE_ROOT_SYNCED)
    } catch (error: Exception) {
      bundleRoot.deleteTree(CURRENT_INCOMING)
      throw IllegalStateException("Failed to switch built-in skill revision", error)
    }
  }

  private fun switchPointer(
    bundleRoot: AndroidPublicationDirectory,
    target: String,
    incoming: String,
    hash: String,
  ) {
    bundleRoot.deleteTree(incoming)
    try {
      val identity = bundleRoot.writeNewFile(incoming, pointerBytes(hash))
      bundleRoot.renameVerifiedFile(incoming, identity, target)
      bundleRoot.sync()
    } catch (error: Exception) {
      bundleRoot.deleteTree(incoming)
      throw IllegalStateException("Failed to switch built-in skill rollback pointer", error)
    }
  }

  private fun cleanupRevisions(revisions: AndroidPublicationDirectory, retained: Set<String>) {
    var changed = false
    for (name in revisions.listNames()) {
      if (name !in retained && HASH_PATTERN.matches(name) && revisions.entryKind(name) == EntryKind.DIRECTORY) {
        revisions.deleteTree(name)
        changed = true
      }
    }
    if (changed) revisions.sync()
  }

  private fun readPointerHash(
    bundleRoot: AndroidPublicationDirectory,
    name: String,
    sync: Boolean = false,
  ): String? {
    if (bundleRoot.entryKind(name) == null) return null
    check(bundleRoot.entryKind(name) == EntryKind.FILE) {
      "Built-in skill $name pointer is not a regular file"
    }
    val hash = bundleRoot.readVerifiedFile(name, sync).toString(StandardCharsets.UTF_8).trim()
    check(HASH_PATTERN.matches(hash)) { "Built-in skill $name pointer hash is invalid" }
    return hash
  }

  private fun pointerBytes(hash: String): ByteArray = hash.toByteArray(StandardCharsets.UTF_8)

  private fun hashTree(
    root: AndroidPublicationDirectory,
    syncFiles: Boolean,
    syncDirectories: Boolean,
  ): String {
    val observed = linkedMapOf<String, SkillAssetType>()
    val files = mutableListOf<Pair<String, ByteArray>>()

    fun visit(directory: AndroidPublicationDirectory, prefix: String) {
      for (name in directory.listNames()) {
        val relativePath = if (prefix.isEmpty()) name else "$prefix/$name"
        when (directory.entryKind(name)) {
          EntryKind.DIRECTORY -> {
            observed[relativePath] = SkillAssetType.DIRECTORY
            directory.openDirectory(name).use { child -> visit(child, relativePath) }
          }
          EntryKind.FILE -> {
            observed[relativePath] = SkillAssetType.FILE
            files += relativePath to directory.readVerifiedFile(name, syncFiles)
          }
          EntryKind.SYMLINK -> error("Published built-in skill revision contains a symlink")
          EntryKind.SPECIAL -> error("Published built-in skill revision contains a special file")
          null -> error("Published built-in skill revision changed during verification")
        }
      }
      if (syncDirectories) directory.sync()
    }

    visit(root, "")
    check(observed == expectedTreeEntries(request.entries)) {
      "Published built-in skill revision entries do not match bundled assets"
    }
    val digest = MessageDigest.getInstance("SHA-256")
    for ((path, bytes) in files.sortedBy { it.first }) updateDigest(digest, path, bytes)
    return digest.digest().toHex()
  }

  private fun verifyRootChain(
    privateRoot: AndroidPublicationDirectory,
    bundleRoot: AndroidPublicationDirectory,
    revisions: AndroidPublicationDirectory,
    revision: AndroidPublicationDirectory,
  ) {
    privateRoot.verifyAbsolutePath(request.privateRoot)
    privateRoot.verifyChildDirectory(BUNDLE_ROOT, bundleRoot.identity)
    bundleRoot.verifyChildDirectory(REVISIONS, revisions.identity)
    revisions.verifyChildDirectory(request.expectedHash, revision.identity)
  }

  private fun installed(changed: Boolean): InstalledSkillBundle = InstalledSkillBundle(
    request.privateRoot
      .resolve(BUNDLE_ROOT)
      .resolve(REVISIONS)
      .resolve(request.expectedHash)
      .toFile(),
    request.expectedHash,
    changed,
  )

  private fun expectedTreeEntries(entries: List<SkillAssetEntry>): Map<String, SkillAssetType> {
    val expected = sortedMapOf<String, SkillAssetType>()
    for (entry in entries) {
      val segments = entry.relativePath.split('/')
      for (index in 1 until segments.size) {
        val directory = segments.take(index).joinToString("/")
        check(expected.putIfAbsent(directory, SkillAssetType.DIRECTORY) != SkillAssetType.FILE) {
          "Built-in skill asset parent is a file"
        }
      }
      val previous = expected.put(entry.relativePath, entry.type)
      check(previous == null || previous == entry.type) { "Built-in skill asset type conflict" }
    }
    return expected
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
    private const val BUNDLE_ROOT = "builtin-skills"
    private const val REVISIONS = "revisions"
    private const val LOCK_FILE = ".install.lock"
    private const val CURRENT = "current"
    private const val CURRENT_INCOMING = ".current.incoming"
    private const val PREVIOUS = "previous"
    private const val PREVIOUS_INCOMING = ".previous.incoming"
    private val HASH_PATTERN = Regex("[0-9a-f]{64}")
  }
}

private class HeldDirectoryTree(val root: AndroidPublicationDirectory) : AutoCloseable {
  private val directories = linkedMapOf("" to root)

  fun ensureDirectory(relativePath: String): AndroidPublicationDirectory {
    if (relativePath.isEmpty()) return root
    var current = root
    val traversed = mutableListOf<String>()
    for (component in relativePath.split('/')) {
      traversed += component
      val key = traversed.joinToString("/")
      current = directories.getOrPut(key) { current.openOrCreateDirectory(component) }
    }
    return current
  }

  override fun close() {
    directories.entries
      .sortedByDescending { entry ->
        if (entry.key.isEmpty()) 0 else entry.key.count { character -> character == '/' } + 1
      }
      .forEach { (_, directory) -> runCatching { directory.close() } }
    directories.clear()
  }
}

private enum class EntryKind {
  FILE,
  DIRECTORY,
  SYMLINK,
  SPECIAL,
}

private data class DirectoryIdentity(val device: Long, val inode: Long)

private data class FileIdentity(
  val device: Long,
  val inode: Long,
  val size: Long,
  val links: Long,
)

private class AndroidPublicationDirectory private constructor(
  private val parcel: ParcelFileDescriptor,
  private val logicalPath: Path,
) : AutoCloseable {
  val identity: DirectoryIdentity = directoryIdentity(Os.fstat(parcel.fileDescriptor))
  private val procPath: String
    get() = "/proc/self/fd/${parcel.fd}"

  init {
    validateAnchor()
  }

  fun openOrCreateDirectory(name: String): AndroidPublicationDirectory {
    validateName(name)
    when (entryKind(name)) {
      null -> {
        try {
          Os.mkdir(childPath(name), DIRECTORY_MODE)
        } catch (error: ErrnoException) {
          if (error.errno != OsConstants.EEXIST) throw error
        }
      }
      EntryKind.DIRECTORY -> {}
      else -> error("Skill publication directory component is not a real directory")
    }
    return openDirectory(name)
  }

  fun createDirectory(name: String): AndroidPublicationDirectory {
    validateName(name)
    check(entryKind(name) == null) { "Skill publication directory already exists" }
    Os.mkdir(childPath(name), DIRECTORY_MODE)
    return openDirectory(name)
  }

  fun openDirectory(name: String): AndroidPublicationDirectory {
    validateName(name)
    validateAnchor()
    val before = Os.lstat(childPath(name))
    val expected = directoryIdentity(before)
    val raw = Os.open(childPath(name), DIRECTORY_FLAGS, 0)
    val opened = fromRaw(raw, logicalPath.resolve(name))
    try {
      check(opened.identity == expected) { "Skill publication directory identity changed while opening" }
      verifyChildDirectory(name, expected)
      validateAnchor()
      return opened
    } catch (error: Exception) {
      opened.close()
      throw error
    }
  }

  fun entryKind(name: String): EntryKind? {
    validateName(name)
    validateAnchor()
    val stat = try {
      Os.lstat(childPath(name))
    } catch (error: ErrnoException) {
      if (error.errno == OsConstants.ENOENT) return null
      throw error
    }
    return when {
      OsConstants.S_ISREG(stat.st_mode) -> EntryKind.FILE
      OsConstants.S_ISDIR(stat.st_mode) -> EntryKind.DIRECTORY
      OsConstants.S_ISLNK(stat.st_mode) -> EntryKind.SYMLINK
      else -> EntryKind.SPECIAL
    }
  }

  fun listNames(): List<String> {
    validateAnchor()
    val names = File(procPath).list()
      ?: throw IllegalStateException("Android /proc/self/fd directory access is unavailable")
    names.forEach(::validateName)
    validateAnchor()
    return names.sorted()
  }

  fun <T> withExclusiveLock(name: String, block: () -> T): T {
    validateName(name)
    validateAnchor()
    val raw = Os.open(
      childPath(name),
      OsConstants.O_RDWR or OsConstants.O_CREAT or OsConstants.O_NOFOLLOW or OsConstants.O_CLOEXEC,
      FILE_MODE,
    )
    val stream = try {
      FileOutputStream(raw)
    } catch (error: Exception) {
      Os.close(raw)
      throw error
    }
    return stream.use { output ->
      val expected = regularIdentity(Os.fstat(raw))
      verifyChildFile(name, expected)
      output.channel.lock().use {
        verifyChildFile(name, expected)
        val result = block()
        verifyChildFile(name, expected)
        result
      }
    }
  }

  fun writeNewFile(name: String, bytes: ByteArray): FileIdentity {
    validateName(name)
    validateAnchor()
    val descriptor = Os.open(
      childPath(name),
      OsConstants.O_WRONLY or OsConstants.O_CREAT or OsConstants.O_EXCL or
        OsConstants.O_NOFOLLOW or OsConstants.O_CLOEXEC,
      FILE_MODE,
    )
    try {
      var offset = 0
      while (offset < bytes.size) {
        val written = Os.write(descriptor, bytes, offset, bytes.size - offset)
        check(written > 0) { "Skill publication file write made no progress" }
        offset += written
      }
      Os.fsync(descriptor)
      val identity = regularIdentity(Os.fstat(descriptor))
      check(identity.size == bytes.size.toLong()) { "Skill publication file size changed during write" }
      verifyChildFile(name, identity)
      return identity
    } finally {
      Os.close(descriptor)
    }
  }

  fun readVerifiedFile(name: String, sync: Boolean): ByteArray {
    validateName(name)
    validateAnchor()
    val access = if (sync) OsConstants.O_RDWR else OsConstants.O_RDONLY
    val descriptor = Os.open(childPath(name), access or OsConstants.O_NOFOLLOW or OsConstants.O_CLOEXEC, 0)
    try {
      val before = regularIdentity(Os.fstat(descriptor))
      verifyChildFile(name, before)
      val output = ByteArrayOutputStream()
      val buffer = ByteArray(8192)
      while (true) {
        val read = Os.read(descriptor, buffer, 0, buffer.size)
        if (read == 0) break
        output.write(buffer, 0, read)
      }
      if (sync) Os.fsync(descriptor)
      val after = regularIdentity(Os.fstat(descriptor))
      check(before == after && after.size == output.size().toLong()) {
        "Skill publication file identity changed during verification"
      }
      verifyChildFile(name, after)
      return output.toByteArray()
    } finally {
      Os.close(descriptor)
    }
  }

  fun renameVerifiedDirectory(source: String, expected: DirectoryIdentity, target: String) {
    verifyChildDirectory(source, expected)
    check(entryKind(target) == null) { "Published skill revision already exists" }
    Os.rename(childPath(source), childPath(target))
    verifyChildDirectory(target, expected)
    validateAnchor()
  }

  fun renameVerifiedFile(source: String, expected: FileIdentity, target: String) {
    verifyChildFile(source, expected)
    Os.rename(childPath(source), childPath(target))
    verifyChildFile(target, expected)
    validateAnchor()
  }

  fun verifyChildDirectory(name: String, expected: DirectoryIdentity) {
    validateName(name)
    val actual = directoryIdentity(Os.lstat(childPath(name)))
    check(actual == expected) { "Skill publication directory identity changed" }
  }

  fun verifyAbsolutePath(path: Path) {
    val actual = directoryIdentity(Os.lstat(path.toString()))
    check(actual == identity) { "App-private publication root identity changed" }
    validateAnchor()
  }

  fun deleteTree(name: String) {
    when (entryKind(name)) {
      null -> return
      EntryKind.DIRECTORY -> {
        val child = openDirectory(name)
        val expected = child.identity
        child.use { opened -> opened.listNames().forEach(opened::deleteTree) }
        verifyChildDirectory(name, expected)
        Os.remove(childPath(name))
      }
      EntryKind.FILE, EntryKind.SYMLINK, EntryKind.SPECIAL -> Os.remove(childPath(name))
    }
    validateAnchor()
  }

  fun sync() {
    validateAnchor()
    Os.fsync(parcel.fileDescriptor)
    validateAnchor()
  }

  override fun close() {
    parcel.close()
  }

  private fun verifyChildFile(name: String, expected: FileIdentity) {
    val actual = regularIdentity(Os.lstat(childPath(name)))
    check(actual == expected) { "Skill publication file identity changed" }
  }

  private fun validateAnchor() {
    val handle = directoryIdentity(Os.fstat(parcel.fileDescriptor))
    val proc = try {
      directoryIdentity(Os.stat(procPath))
    } catch (error: Exception) {
      throw IllegalStateException("Android /proc/self/fd directory access is unavailable", error)
    }
    check(handle == identity && proc == identity) { "Skill publication directory handle identity changed" }
  }

  private fun childPath(name: String): String {
    validateName(name)
    return "$procPath/$name"
  }

  companion object {
    private const val DIRECTORY_MODE = 0x1c0
    private const val FILE_MODE = 0x180
    private val DIRECTORY_FLAGS =
      OsConstants.O_RDONLY or OsConstants.O_NOFOLLOW or OsConstants.O_CLOEXEC

    fun openRoot(path: Path): AndroidPublicationDirectory {
      val raw = Os.open(path.toString(), DIRECTORY_FLAGS, 0)
      val opened = fromRaw(raw, path)
      try {
        opened.verifyAbsolutePath(path)
        return opened
      } catch (error: Exception) {
        opened.close()
        throw error
      }
    }

    private fun fromRaw(raw: java.io.FileDescriptor, path: Path): AndroidPublicationDirectory {
      val parcel = try {
        ParcelFileDescriptor.dup(raw)
      } finally {
        Os.close(raw)
      }
      return AndroidPublicationDirectory(parcel, path)
    }

    private fun validateName(name: String) {
      require(
        name.isNotEmpty() && name != "." && name != ".." && '/' !in name && '\u0000' !in name,
      ) { "Skill publication operations require one safe relative path component" }
    }

    private fun directoryIdentity(stat: StructStat): DirectoryIdentity {
      check(OsConstants.S_ISDIR(stat.st_mode)) { "Skill publication component is not a real directory" }
      return DirectoryIdentity(stat.st_dev, stat.st_ino)
    }

    private fun regularIdentity(stat: StructStat): FileIdentity {
      check(OsConstants.S_ISREG(stat.st_mode)) { "Skill publication component is not a regular file" }
      check(stat.st_nlink == 1L) { "Skill publication file must have exactly one link" }
      return FileIdentity(stat.st_dev, stat.st_ino, stat.st_size, stat.st_nlink)
    }
  }
}
