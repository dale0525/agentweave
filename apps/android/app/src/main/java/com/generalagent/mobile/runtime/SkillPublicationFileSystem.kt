package com.generalagent.mobile.runtime

import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.channels.FileChannel
import java.nio.file.AtomicMoveNotSupportedException
import java.nio.file.FileAlreadyExistsException
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.nio.file.StandardOpenOption
import java.nio.file.attribute.BasicFileAttributes

internal enum class SkillPublicationFaultPoint {
  FILES_SYNCED,
  DIRECTORIES_SYNCED,
  REVISION_RENAMED,
  REVISIONS_SYNCED,
  CURRENT_TEMP_SYNCED,
  CURRENT_RENAMED,
  BUNDLE_ROOT_SYNCED,
}

internal fun interface SkillPublicationFaults {
  fun after(point: SkillPublicationFaultPoint)

  companion object {
    val NONE = SkillPublicationFaults {}
  }
}

internal data class SkillFileIdentity(
  val device: Long?,
  val inode: Long?,
  val fileKey: String?,
  val size: Long,
  val links: Long,
)

internal data class DirectoryBoundSkillPublicationRequest(
  val privateRoot: Path,
  val expectedHash: String,
  val entries: List<SkillAssetEntry>,
  val assets: SkillAssetSource,
  val faults: SkillPublicationFaults,
)

internal interface DirectoryBoundSkillPublication {
  fun installDirectoryBound(request: DirectoryBoundSkillPublicationRequest): InstalledSkillBundle
}

interface SkillPublicationFileSystem {
  fun <T> withExclusiveLock(path: Path, block: () -> T): T

  fun writeNewFile(path: Path, bytes: ByteArray)

  fun readVerifiedFile(path: Path, sync: Boolean = false): ByteArray

  fun directoryIdentity(path: Path): String

  fun syncDirectory(path: Path)

  fun atomicMove(source: Path, target: Path, replace: Boolean)
}

internal open class JvmSkillPublicationFileSystem(
  private val afterVerifiedOpen: (Path) -> Unit = {},
) : SkillPublicationFileSystem {
  override fun <T> withExclusiveLock(path: Path, block: () -> T): T {
    val channel = openLockFile(path)
    return channel.use { opened ->
      val identity = regularIdentity(path)
      opened.lock().use {
        check(regularIdentity(path) == identity) { "Built-in skill installer lock identity changed" }
        val result = block()
        check(regularIdentity(path) == identity) { "Built-in skill installer lock identity changed" }
        result
      }
    }
  }

  override fun writeNewFile(path: Path, bytes: ByteArray) {
    FileChannel.open(
      path,
      StandardOpenOption.CREATE_NEW,
      StandardOpenOption.WRITE,
      LinkOption.NOFOLLOW_LINKS,
    ).use { channel ->
      var offset = 0
      while (offset < bytes.size) {
        offset += channel.write(ByteBuffer.wrap(bytes, offset, bytes.size - offset))
      }
      channel.force(true)
      val identity = regularIdentity(path)
      check(identity.size == channel.size()) { "Published file identity changed during write" }
    }
  }

  override fun readVerifiedFile(path: Path, sync: Boolean): ByteArray {
    val channel = if (sync) {
      FileChannel.open(
        path,
        StandardOpenOption.READ,
        StandardOpenOption.WRITE,
        LinkOption.NOFOLLOW_LINKS,
      )
    } else {
      FileChannel.open(path, StandardOpenOption.READ, LinkOption.NOFOLLOW_LINKS)
    }
    return channel.use { opened ->
      val before = regularIdentity(path)
      check(before.size == opened.size()) { "Published file identity changed before verification" }
      afterVerifiedOpen(path)
      val output = ByteArrayOutputStream()
      val buffer = ByteBuffer.allocate(8192)
      while (opened.read(buffer) >= 0) {
        if (buffer.position() == 0) continue
        output.write(buffer.array(), 0, buffer.position())
        buffer.clear()
      }
      if (sync) opened.force(true)
      val after = regularIdentity(path)
      check(before == after && after.size == opened.size()) {
        "Published file identity changed during verification"
      }
      output.toByteArray()
    }
  }

  override fun directoryIdentity(path: Path): String {
    val attributes = Files.readAttributes(path, BasicFileAttributes::class.java, LinkOption.NOFOLLOW_LINKS)
    check(attributes.isDirectory && !attributes.isSymbolicLink) {
      "Skill publication directory must be a real directory"
    }
    return attributes.fileKey()?.toString()
      ?: throw IllegalStateException("Skill publication directory identity is unavailable")
  }

  override fun syncDirectory(path: Path) {
    FileChannel.open(path, StandardOpenOption.READ, LinkOption.NOFOLLOW_LINKS).use { channel ->
      channel.force(true)
    }
  }

  override fun atomicMove(source: Path, target: Path, replace: Boolean) {
    val options = if (replace) {
      arrayOf(StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
    } else {
      arrayOf(StandardCopyOption.ATOMIC_MOVE)
    }
    try {
      Files.move(source, target, *options)
    } catch (error: AtomicMoveNotSupportedException) {
      throw IllegalStateException("App-private storage does not support atomic publication", error)
    }
  }

  private fun openLockFile(path: Path): FileChannel {
    try {
      FileChannel.open(
        path,
        StandardOpenOption.CREATE_NEW,
        StandardOpenOption.WRITE,
        LinkOption.NOFOLLOW_LINKS,
      ).use { it.force(true) }
    } catch (_: FileAlreadyExistsException) {
      // Existing lock identity is verified after opening without following links.
    }
    val channel = try {
      FileChannel.open(path, StandardOpenOption.WRITE, LinkOption.NOFOLLOW_LINKS)
    } catch (error: Exception) {
      throw IllegalStateException("Failed to open built-in skill installer lock", error)
    }
    return try {
      val identity = regularIdentity(path)
      check(identity.size == channel.size()) { "Built-in skill installer lock identity changed" }
      channel
    } catch (error: Exception) {
      channel.close()
      throw error
    }
  }

  protected open fun regularIdentity(path: Path): SkillFileIdentity {
    val attributes = Files.readAttributes(path, BasicFileAttributes::class.java, LinkOption.NOFOLLOW_LINKS)
    check(attributes.isRegularFile && !attributes.isSymbolicLink) {
      "Skill publication file must be a regular file"
    }
    val links = try {
      (Files.getAttribute(path, "unix:nlink", LinkOption.NOFOLLOW_LINKS) as Number).toLong()
    } catch (error: Exception) {
      throw IllegalStateException("Skill publication link count is unavailable", error)
    }
    check(links == 1L) { "Skill publication file must have exactly one link" }
    return SkillFileIdentity(
      device = null,
      inode = null,
      fileKey = attributes.fileKey()?.toString()
        ?: throw IllegalStateException("Skill publication file identity is unavailable"),
      size = attributes.size(),
      links = links,
    )
  }
}

internal class AndroidSkillPublicationFileSystem(
  private val hooks: AndroidSkillPublicationHooks = AndroidSkillPublicationHooks.NONE,
) : SkillPublicationFileSystem, DirectoryBoundSkillPublication {
  override fun installDirectoryBound(request: DirectoryBoundSkillPublicationRequest): InstalledSkillBundle =
    AndroidDirectoryBoundSkillPublisher(request, hooks).install()

  override fun <T> withExclusiveLock(path: Path, block: () -> T): T = pathApiUnavailable()

  override fun writeNewFile(path: Path, bytes: ByteArray): Unit = pathApiUnavailable()

  override fun readVerifiedFile(path: Path, sync: Boolean): ByteArray = pathApiUnavailable()

  override fun directoryIdentity(path: Path): String = pathApiUnavailable()

  override fun syncDirectory(path: Path): Unit = pathApiUnavailable()

  override fun atomicMove(source: Path, target: Path, replace: Boolean): Unit = pathApiUnavailable()

  private fun <T> pathApiUnavailable(): T =
    throw IllegalStateException("Android skill publication requires directory-bound operations")
}
