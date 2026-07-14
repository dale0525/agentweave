package com.agentweave.mobile.runtime

import java.nio.charset.StandardCharsets

internal data class SkillPointerTransaction(
  val oldCurrent: String,
  val oldPrevious: String?,
  val target: String,
) {
  init {
    requireHash(oldCurrent, "old current")
    oldPrevious?.let { requireHash(it, "old previous") }
    requireHash(target, "target")
  }

  fun encode(): ByteArray = buildString {
    appendLine(HEADER)
    appendLine("old_current=$oldCurrent")
    appendLine("old_previous=${oldPrevious ?: NONE}")
    appendLine("target=$target")
  }.toByteArray(StandardCharsets.US_ASCII)

  companion object {
    private const val HEADER = "agentweave-skill-pointer-transaction-v1"
    private const val NONE = "-"
    private const val MAX_BYTES = 512
    private val HASH_PATTERN = Regex("[0-9a-f]{64}")

    fun decode(bytes: ByteArray): SkillPointerTransaction {
      require(bytes.size <= MAX_BYTES) { "Built-in skill pointer transaction is too large" }
      val lines = bytes.toString(StandardCharsets.US_ASCII).trimEnd('\n').split('\n')
      require(lines.size == 4 && lines[0] == HEADER) {
        "Built-in skill pointer transaction is invalid"
      }
      val oldCurrent = field(lines[1], "old_current")
      val oldPrevious = field(lines[2], "old_previous").takeUnless { it == NONE }
      val target = field(lines[3], "target")
      return SkillPointerTransaction(oldCurrent, oldPrevious, target)
    }

    private fun field(line: String, name: String): String {
      val prefix = "$name="
      require(line.startsWith(prefix)) { "Built-in skill pointer transaction is invalid" }
      return line.removePrefix(prefix)
    }

    private fun requireHash(hash: String, label: String) {
      require(HASH_PATTERN.matches(hash)) {
        "Built-in skill pointer transaction $label hash is invalid"
      }
    }
  }
}

internal enum class SkillPointerRecoveryAction {
  ABORT,
  FINALIZE,
  RESUME,
}

internal fun skillPointerRecoveryAction(
  transaction: SkillPointerTransaction,
  current: String?,
  previous: String?,
  expected: String,
): SkillPointerRecoveryAction {
  check(current != null) { "Built-in skill pointer transaction exists without current" }
  if (current == transaction.target) {
    check(previous == transaction.oldCurrent) {
      "Committed built-in skill pointer transaction has an inconsistent previous pointer"
    }
    return SkillPointerRecoveryAction.FINALIZE
  }
  check(current == transaction.oldCurrent) {
    "Built-in skill pointer transaction current pointer is inconsistent"
  }
  check(previous == transaction.oldPrevious || previous == transaction.oldCurrent) {
    "Built-in skill pointer transaction previous pointer is inconsistent"
  }
  return if (expected == transaction.target) {
    SkillPointerRecoveryAction.RESUME
  } else {
    SkillPointerRecoveryAction.ABORT
  }
}
