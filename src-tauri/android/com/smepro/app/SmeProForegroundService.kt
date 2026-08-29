package com.smepro.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat

/**
 * Keeps this process alive under Android's background execution limits
 * (API 26+) so the app's embedded local API server — a plain background
 * thread started from Rust (see src-tauri/src/android_service.rs) — is
 * not reclaimed by the OS a few minutes after the app leaves the
 * foreground. This is the gap that file's own doc comment has flagged
 * as open: "a plain background thread with no foreground service and
 * no persistent notification is reclaimed by the OS."
 *
 * Deliberately does nothing beyond holding a foreground-priority
 * notification. It does NOT start, stop, or otherwise manage the Rust
 * HTTP server thread — that already starts on its own from lib.rs's
 * `run()` regardless of this service. This service's only job is to
 * raise the whole process's scheduling priority so that already-running
 * thread survives being backgrounded, not to control it.
 */
class SmeProForegroundService : Service() {
    private val channelId = "smepro_background_service"
    private val notificationId = 1

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        startForeground(notificationId, buildNotification())
        // START_STICKY: if Android still kills this process under real
        // memory pressure despite the foreground priority, ask the OS
        // to recreate this service (with a null Intent) once resources
        // free up, instead of leaving it dead until the user manually
        // reopens the app.
        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun buildNotification(): Notification {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                channelId,
                "SME Pro background service",
                // MIN, not the default — this notification exists only to
                // satisfy Android's foreground-service requirement, not to
                // alert the user of anything. No sound, no visible badge,
                // minimized in the notification shade.
                NotificationManager.IMPORTANCE_MIN
            )
            channel.description = "Keeps SME Pro's local data server available while the app is open"
            getSystemService(NotificationManager::class.java)?.createNotificationChannel(channel)
        }
        return NotificationCompat.Builder(this, channelId)
            .setContentTitle("SME Pro is running")
            .setContentText("Keeping your local data server available.")
            .setSmallIcon(applicationInfo.icon)
            .setPriority(NotificationCompat.PRIORITY_MIN)
            .setOngoing(true)
            .build()
    }
}
