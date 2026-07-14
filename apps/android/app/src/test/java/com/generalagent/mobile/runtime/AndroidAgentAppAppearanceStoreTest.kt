package com.generalagent.mobile.runtime

import android.content.Context
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class AndroidAgentAppAppearanceStoreTest {
  private val context: Context = RuntimeEnvironment.getApplication()

  @After
  fun clearPreferences() {
    context.getSharedPreferences("generalagent.appearance.v1", Context.MODE_PRIVATE)
      .edit()
      .clear()
      .commit()
  }

  @Test
  fun selectedThemePersistsAndRejectsThemesOutsideThePackageManifest() {
    val store = AndroidAgentAppAppearanceStore(context)
    assertTrue(store.appearance.themes.size >= 19)
    assertEquals(DEFAULT_ANDROID_THEME_ID, store.selectedTheme())

    assertTrue(store.selectTheme("vscode.light-2026"))
    assertEquals("vscode.light-2026", AndroidAgentAppAppearanceStore(context).selectedTheme())
    assertFalse(store.selectTheme("com.example.not-packaged"))
    assertEquals("vscode.light-2026", store.selectedTheme())
  }
}
