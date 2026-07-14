package com.generalagent.mobile

import android.app.Application
import com.generalagent.mobile.runtime.AutomationScheduling

class GeneralAgentApplication : Application() {
  override fun onCreate() {
    super.onCreate()
    AutomationScheduling.ensureScheduled(this)
  }
}
