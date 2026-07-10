package com.generalagent.mobile.secrets

import java.nio.charset.StandardCharsets
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
    val store = AndroidKeystoreModelSecretStore(root) { key }

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
  }

  @Test
  fun encryptedStoreRejectsTamperedCiphertext() {
    val root = temporaryFolder.newFolder("tampered-secrets")
    val key = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
    val store = AndroidKeystoreModelSecretStore(root) { key }
    store.saveSecret("model.default", "sk-test")
    val encryptedFile = root.listFiles().orEmpty().single()
    val bytes = encryptedFile.readBytes()
    bytes[bytes.lastIndex] = (bytes.last().toInt() xor 0x01).toByte()
    encryptedFile.writeBytes(bytes)

    assertThrows(ModelSecretStoreException::class.java) {
      store.loadSecret("model.default")
    }
  }
}
