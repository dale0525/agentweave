package com.agentweave.mobile

import android.app.Application
import com.agentweave.mobile.runtime.AutomationScheduling

class AgentWeaveApplication : Application() {
  override fun onCreate() {
    super.onCreate()
    AutomationScheduling.ensureScheduled(this)
  }
}
