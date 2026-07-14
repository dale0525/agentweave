package com.agentweave.mobile.secrets

import java.io.File
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import javax.crypto.KeyGenerator
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class ModelSecretStoreTest {
  @get:Rule
  val temporaryFolder = TemporaryFolder()

  @Test
  fun inMemoryStoreSavesLoadsAndDeletesSecret() {
    val store = InMemoryModelSecretStore()

    store.saveSecret("model.default", "sk-test")

    assertEquals("sk-test", store.loadSecret("model.default"))
    store.deleteSecret("model.default")
    assertNull(store.loadSecret("model.default"))
  }

  @Test
  fun encryptedStoreRoundTripsWithoutPersistingPlaintext() {
    val root = temporaryFolder.newFolder("model-secrets")
    val key = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
    val syncedDirectories = mutableListOf<File>()
    val store = AndroidKeystoreModelSecretStore(root, { key }, syncedDirectories::add)

    store.saveSecret("model.default", "sk-first")
    store.saveSecret("model.default", "sk-replaced")

    assertEquals("sk-replaced", store.loadSecret("model.default"))
    val encryptedFile = root.listFiles().orEmpty().single()
    assertFalse(encryptedFile.name.contains("model.default"))
    assertFalse(
      encryptedFile
        .readBytes()
        .toString(StandardCharsets.UTF_8)
        .contains("sk-replaced"),
    )

    store.deleteSecret("model.default")
    assertNull(store.loadSecret("model.default"))
    assertEquals(listOf(root, root, root), syncedDirectories)
  }

  @Test
  fun encryptedStoreUsesFreshIvForEverySave() {
    val root = temporaryFolder.newFolder("fresh-iv")
    val key = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
    val store = AndroidKeystoreModelSecretStore(root, { key }) {}

    store.saveSecret("model.default", "sk-same")
    val firstEnvelope = root.listFiles().orEmpty().single().readBytes()
    store.saveSecret("model.default", "sk-same")
    val secondEnvelope = root.listFiles().orEmpty().single().readBytes()

    assertFalse(firstEnvelope.contentEquals(secondEnvelope))
  }

  @Test
  fun encryptedStoreRejectsTamperedCiphertext() {
    val root = temporaryFolder.newFolder("tampered-secrets")
    val key = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
    val store = AndroidKeystoreModelSecretStore(root, { key }) {}
    store.saveSecret("model.default", "sk-test")
    val encryptedFile = root.listFiles().orEmpty().single()
    val bytes = encryptedFile.readBytes()
    bytes[bytes.lastIndex] = (bytes.last().toInt() xor 0x01).toByte()
    encryptedFile.writeBytes(bytes)

    assertThrows(ModelSecretStoreException::class.java) {
      store.loadSecret("model.default")
    }
  }

  @Test
  fun encryptedStoreRejectsSwappedSecretIdAndMalformedEnvelope() {
    val root = temporaryFolder.newFolder("invalid-envelopes")
    val key = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
    val store = AndroidKeystoreModelSecretStore(root, { key }) {}
    store.saveSecret("model.first", "sk-first")
    val firstFile = root.listFiles().orEmpty().single()
    val validEnvelope = firstFile.readBytes()
    store.saveSecret("model.second", "sk-second")
    val secondFile = root.listFiles().orEmpty().first { it != firstFile }
    Files.copy(firstFile.toPath(), secondFile.toPath(), StandardCopyOption.REPLACE_EXISTING)

    assertThrows(ModelSecretStoreException::class.java) {
      store.loadSecret("model.second")
    }

    val malformedPayloads = listOf(
      validEnvelope.clone().also { it[0] = 0x00 },
      validEnvelope.clone().also { it[4] = 0x02 },
      validEnvelope.clone().also { it[5] = 0x01 },
      validEnvelope.copyOf(8),
    )
    malformedPayloads.forEach { payload ->
      secondFile.writeBytes(payload)
      assertThrows(ModelSecretStoreException::class.java) {
        store.loadSecret("model.second")
      }
    }
  }
}
