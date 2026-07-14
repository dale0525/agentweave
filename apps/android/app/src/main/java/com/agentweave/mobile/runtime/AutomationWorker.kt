package com.agentweave.mobile.runtime

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

private const val AUTOMATION_WORK = "agentweave-foundation-automation-v1"
private const val NOTIFICATION_CHANNEL = "agentweave-foundation"
private const val NOTIFICATION_CHANNEL_NAME = "Agent updates"
private const val WORKER_ID = "android-workmanager"

object AutomationScheduling {
  fun ensureScheduled(context: Context): Boolean {
    val request = PeriodicWorkRequestBuilder<AgentAutomationWorker>(15, TimeUnit.MINUTES)
      .setConstraints(Constraints.Builder().build())
      .build()
    val manager = runCatching { WorkManager.getInstance(context) }
      .getOrElse { return false }
    manager.enqueueUniquePeriodicWork(
      AUTOMATION_WORK,
      ExistingPeriodicWorkPolicy.UPDATE,
      request,
    )
    return true
  }
}

class AgentAutomationWorker(
  appContext: Context,
  parameters: WorkerParameters,
) : CoroutineWorker(appContext, parameters) {
  override suspend fun doWork(): Result = withContext(Dispatchers.IO) {
    val client = runCatching { RuntimeBridge(applicationContext).load() }
      .getOrElse { return@withContext Result.retry() }
    try {
      client.runSchedulerTick()
      if (!notificationsAllowed(applicationContext)) return@withContext Result.success()
      ensureNotificationChannel(applicationContext)
      client.claimNotifications(WORKER_ID).forEach { notification ->
        deliver(client, notification)
      }
      Result.success()
    } catch (_: RuntimeBridgeException) {
      Result.retry()
    } finally {
      runCatching { client.close() }
    }
  }

  private fun deliver(client: RuntimeClient, notification: RuntimeNotification) {
    if (notification.channel !in setOf("android", "mobile", "local")) {
      client.finishNotificationUncertain(
        notification.notificationId,
        WORKER_ID,
        "notification channel is not available on Android",
      )
      return
    }
    try {
      val manager = applicationContext.getSystemService(NotificationManager::class.java)
      val platformId = stableNotificationId(notification.notificationId)
      val rendered = NotificationCompat.Builder(applicationContext, NOTIFICATION_CHANNEL)
        .setSmallIcon(android.R.drawable.ic_dialog_info)
        .setContentTitle(notification.title)
        .setContentText(notification.body)
        .setStyle(NotificationCompat.BigTextStyle().bigText(notification.body))
        .setAutoCancel(true)
        .build()
      manager.notify(platformId, rendered)
      client.finishNotificationDelivered(
        notification.notificationId,
        WORKER_ID,
        "android:$platformId",
      )
    } catch (failure: Throwable) {
      client.finishNotificationUncertain(
        notification.notificationId,
        WORKER_ID,
        failure.message ?: "Android notification delivery failed",
      )
    }
  }
}

internal fun stableNotificationId(value: String): Int = value.hashCode() and Int.MAX_VALUE

private fun notificationsAllowed(context: Context): Boolean =
  Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
    ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) ==
    PackageManager.PERMISSION_GRANTED

private fun ensureNotificationChannel(context: Context) {
  val manager = context.getSystemService(NotificationManager::class.java)
  manager.createNotificationChannel(
    NotificationChannel(
      NOTIFICATION_CHANNEL,
      NOTIFICATION_CHANNEL_NAME,
      NotificationManager.IMPORTANCE_DEFAULT,
    ),
  )
}
