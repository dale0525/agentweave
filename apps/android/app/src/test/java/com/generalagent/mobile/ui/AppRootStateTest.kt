package com.generalagent.mobile.ui

import com.generalagent.mobile.model.ModelSettings
import com.generalagent.mobile.runtime.RuntimeMessage
import com.generalagent.mobile.runtime.RuntimeModelConfig
import com.generalagent.mobile.runtime.RuntimeSkill
import com.generalagent.mobile.secrets.InMemoryModelSecretStore
import com.generalagent.mobile.secrets.ModelSecretStore
import com.generalagent.mobile.secrets.ModelSecretStoreException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.fail
import org.junit.Test

class AppRootStateTest {
  @Test
  fun tabsExposeMvpScreensInStableOrder() {
    assertEquals(
      listOf(AppTab.Chat, AppTab.Settings, AppTab.Skills, AppTab.Diagnostics),
      AppTab.entries,
    )
    assertEquals(
      listOf("Chat", "Settings", "Skills", "Diagnostics"),
      AppTab.entries.map { it.label },
    )
  }

  @Test
  fun skillRowsPreserveCapabilityReasons() {
    val skills = listOf(
      RuntimeSkill("web", "Web browser", "Browse", false, "Missing required capability: browser.headless"),
      RuntimeSkill("shell", "Shell tools", "Run commands", false, "Missing required capability: shell.process"),
      RuntimeSkill("desktop", "Desktop automation", "Automate", false, "Not supported on Android"),
    )
    assertEquals(
      listOf(
        "Missing required capability: browser.headless",
        "Missing required capability: shell.process",
        "Not supported on Android",
      ),
      androidSkillRows(skills).filterNot { it.available }.map { it.detail },
    )
  }

  @Test
  fun diagnosticsPreserveExactCapabilityIds() {
    assertEquals(
      listOf(
        "network.http",
        "filesystem.app_data",
        "secure_storage",
        "model.http_provider",
      ),
      androidDiagnosticCapabilityIds(),
    )
  }

  @Test
  fun providerChangeDoesNotReuseAnotherProvidersSecret() {
    assertNull(
      modelSecretReferenceForSave(
        providerId = "local",
        currentSecretId = "model.openai.default",
        hasSavedSecret = true,
        hasNewSecret = false,
      ),
    )
    assertEquals(
      "model.local.default",
      modelSecretReferenceForSave(
        providerId = "local",
        currentSecretId = "model.openai.default",
        hasSavedSecret = true,
        hasNewSecret = true,
      ),
    )
  }

  @Test
  fun providerOptionsMatchSupportedGatewayProtocols() {
    assertEquals(listOf("openai", "local"), androidProviderIds())
  }

  @Test
  fun selectingCurrentProviderDoesNotResetSecretState() {
    assertFalse(providerSelectionChanges("openai", "openai"))
    assertEquals(true, providerSelectionChanges("openai", "local"))
  }

  @Test
  fun activeNavigationGeometryMatchesStitchRoutes() {
    assertEquals(NavigationSize(64, 48), AppTab.Chat.activeNavigationSize())
    assertEquals(NavigationSize(72, 60), AppTab.Settings.activeNavigationSize())
    assertEquals(NavigationSize(64, 48), AppTab.Skills.activeNavigationSize())
    assertEquals(NavigationSize(72, 64), AppTab.Diagnostics.activeNavigationSize())
  }

  @Test
  fun pendingApiKeyOnlyBelongsToProviderThatCapturedIt() {
    assertEquals("sk-openai", pendingApiKeyForProvider("openai", "openai", " sk-openai "))
    assertNull(pendingApiKeyForProvider("local", "openai", " sk-openai "))
  }

  @Test
  fun settingsSaveWaitsForLookupAndRuntimeTurn() {
    assertFalse(canSaveModelSettings(saving = false, initialConfigReady = false, secretLookupReady = true, hasReplacementSecret = false, runtimeBusy = false))
    assertFalse(canSaveModelSettings(saving = false, initialConfigReady = true, secretLookupReady = false, hasReplacementSecret = false, runtimeBusy = false))
    assertEquals(true, canSaveModelSettings(saving = false, initialConfigReady = true, secretLookupReady = false, hasReplacementSecret = true, runtimeBusy = false))
    assertFalse(canSaveModelSettings(saving = true, initialConfigReady = true, secretLookupReady = true, hasReplacementSecret = false, runtimeBusy = false))
    assertFalse(canSaveModelSettings(saving = false, initialConfigReady = true, secretLookupReady = true, hasReplacementSecret = false, runtimeBusy = true))
    assertEquals(
      true,
      canSaveModelSettings(saving = false, initialConfigReady = true, secretLookupReady = true, hasReplacementSecret = false, runtimeBusy = false),
    )
  }

  @Test
  fun staleProviderLookupCannotUnlockANewerLookupForSameProvider() {
    assertFalse(isCurrentSecretLookup(3, 1, "openai", "openai"))
    assertEquals(true, isCurrentSecretLookup(3, 3, "openai", "openai"))
  }

  @Test
  fun settingsSaveBlocksTabNavigationUntilCommitFinishes() {
    assertEquals(AppTab.Settings, admittedAppTab(AppTab.Settings, AppTab.Chat, settingsSaving = true))
    assertEquals(AppTab.Chat, admittedAppTab(AppTab.Settings, AppTab.Chat, settingsSaving = false))
  }

  @Test
  fun runtimeTurnGateRejectsConcurrentAdmissionUntilFinished() {
    val gate = RuntimeTurnGate()
    gate.updateDraft("hello")

    assertEquals(true, gate.tryBegin())
    assertFalse(gate.tryBegin())
    assertEquals(true, gate.inFlight.value)
    assertEquals("hello", gate.draft.value)

    gate.finish(refreshHistory = true)

    assertFalse(gate.inFlight.value)
    assertEquals(1, gate.completionVersion.value)
    assertEquals(true, gate.tryBegin())
    gate.close()
  }

  @Test
  fun runtimeTurnGateExposesPendingUserMessageUntilFinished() {
    val gate = RuntimeTurnGate()
    val pending = RuntimeMessage(
      id = "pending-user",
      sessionId = "session-1",
      role = "user",
      content = "hello",
      createdAt = "",
    )

    assertEquals(true, gate.tryBegin(pending))
    assertEquals(pending, gate.pendingUserMessage.value)

    gate.finish()

    assertNull(gate.pendingUserMessage.value)
    gate.close()
  }

  @Test
  fun persistedUserMessageReplacesOnlyTheMatchingPendingMessage() {
    val previous = RuntimeMessage("user-1", "session-1", "user", "hello", "before")
    val persisted = RuntimeMessage("user-2", "session-1", "user", "hello", "after")
    val pending = RuntimeMessage("pending-user", "session-1", "user", "hello", "")

    assertEquals(
      listOf(previous, pending),
      chatMessagesForDisplay(listOf(previous), pending, setOf(previous.id)),
    )
    assertEquals(
      listOf(previous, persisted),
      chatMessagesForDisplay(listOf(previous, persisted), pending, setOf(previous.id)),
    )
  }

  @Test
  fun runtimeTurnGateRetainsTurnErrorUntilNextAdmission() {
    val gate = RuntimeTurnGate()

    assertEquals(true, gate.tryBegin())
    gate.reportTurnError("model unavailable")
    gate.finish()

    assertEquals("model unavailable", gate.turnErrorMessage.value)
    assertEquals(true, gate.tryBegin())
    assertNull(gate.turnErrorMessage.value)
    gate.finish()
    gate.close()
  }

  @Test
  fun runtimeSettingsGateRejectsConcurrentSavesUntilFinished() {
    val gate = RuntimeSettingsGate()

    assertEquals(true, gate.tryBegin())
    assertFalse(gate.tryBegin())
    assertEquals(true, gate.inFlight.value)

    gate.finish()

    assertFalse(gate.inFlight.value)
    assertEquals(1, gate.completionVersion.value)
    assertEquals(true, gate.tryBegin())
    gate.close()
  }

  @Test
  fun failedConfigSaveRestoresPreviousSecret() {
    val store = InMemoryModelSecretStore().apply {
      saveSecret("model.openai.default", "sk-old")
    }
    val settings = modelSettings(apiKey = "sk-new")

    try {
      persistModelSettings(settings, store) { error("database unavailable") }
      fail("expected save failure")
    } catch (_: IllegalStateException) {
      // Expected.
    }

    assertEquals("sk-old", store.loadSecret("model.openai.default"))
  }

  @Test
  fun failedFirstConfigSaveDeletesNewSecret() {
    val store = InMemoryModelSecretStore()

    try {
      persistModelSettings(modelSettings(apiKey = "sk-new"), store) {
        error("database unavailable")
      }
      fail("expected save failure")
    } catch (_: IllegalStateException) {
      // Expected.
    }

    assertFalse(store.loadSecret("model.openai.default") != null)
  }

  @Test
  fun replacementSecretRecoversFromUnreadableStoredSecret() {
    var stored: String? = "corrupt"
    val store = object : com.generalagent.mobile.secrets.ModelSecretStore {
      override fun saveSecret(secretId: String, value: String) {
        stored = value
      }

      override fun loadSecret(secretId: String): String? {
        if (stored == "corrupt") {
          throw com.generalagent.mobile.secrets.ModelSecretStoreException("ciphertext is unreadable")
        }
        return stored
      }

      override fun deleteSecret(secretId: String) {
        stored = null
      }
    }

    val saved = persistModelSettings(modelSettings(apiKey = "sk-replacement"), store) { }

    assertEquals(true, saved)
    assertEquals("sk-replacement", stored)
  }

  @Test
  fun unreadableStoredSecretDoesNotBlockInitialSettings() {
    val config = RuntimeModelConfig(
      providerId = "openai",
      providerName = "OpenAI-compatible",
      endpointType = "responses",
      baseUrl = "https://api.openai.com/v1",
      modelName = "gpt-5.4",
      secretId = "model.openai.default",
    )
    val store = object : ModelSecretStore {
      override fun saveSecret(secretId: String, value: String) = Unit

      override fun loadSecret(secretId: String): String? {
        throw ModelSecretStoreException("ciphertext is unreadable")
      }

      override fun deleteSecret(secretId: String) = Unit
    }

    val loaded = loadInitialModelSettings({ config }, store)

    assertEquals(config, loaded.config)
    assertFalse(loaded.secretSaved)
    assertEquals("ciphertext is unreadable", loaded.secretLookupError)
  }

  private fun modelSettings(apiKey: String) = ModelSettings(
    providerId = "openai",
    providerName = "OpenAI-compatible",
    endpointType = "responses",
    baseUrl = "https://api.openai.com/v1",
    modelName = "gpt-5.4",
    secretId = "model.openai.default",
    apiKey = apiKey,
  )
}
