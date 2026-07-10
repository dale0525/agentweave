package com.generalagent.mobile.secrets

import android.security.keystore.KeyInfo
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.security.KeyStore
import java.util.UUID
import javax.crypto.SecretKey
import javax.crypto.SecretKeyFactory
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Test

class AndroidKeystoreModelSecretStoreInstrumentedTest {
  @Test
  fun storeUsesNonExportableAndroidKeystoreKeyAndNoBackupStorage() {
    val context = InstrumentationRegistry.getInstrumentation().targetContext
    val directory = File(context.noBackupFilesDir, "model-secrets")
    val before = directory.listFiles().orEmpty().mapTo(mutableSetOf()) { it.name }
    val secretId = "model.instrumentation.${UUID.randomUUID()}"
    val secretValue = "sk-instrumentation-${UUID.randomUUID()}"
    val store = AndroidKeystoreModelSecretStore(context)

    try {
      store.saveSecret(secretId, secretValue)

      assertEquals(secretValue, store.loadSecret(secretId))
      val created = directory.listFiles().orEmpty().filter { it.name !in before }
      assertEquals(1, created.size)
      assertFalse(created.single().readText().contains(secretValue))

      val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
      val key = keyStore.getKey("com.generalagent.mobile.model-secrets.v1", null) as SecretKey
      assertNull(key.encoded)
      val keyInfo = SecretKeyFactory.getInstance(key.algorithm, "AndroidKeyStore")
        .getKeySpec(key, KeyInfo::class.java) as KeyInfo
      assertEquals(256, keyInfo.keySize)
    } finally {
      store.deleteSecret(secretId)
    }
  }
}
