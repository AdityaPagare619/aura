package dev.aura.v4

import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager
import android.os.Build
import android.os.StrictMode
import android.util.Log

/**
 * Custom [Application] class for AURA v4.
 *
 * Responsibilities:
 * - Create the foreground-service notification channel (required on O+).
 * - Optionally enable StrictMode in debug builds.
 * - Hold a static application context for subsystems that need it.
 */
class AuraApplication : Application() {

    companion object {
        private const val TAG = "AuraApplication"

        /** Notification channel used by [AuraForegroundService]. */
        const val CHANNEL_FOREGROUND = "aura_foreground"

        /** Notification channel used for daemon status / alerts. */
        const val CHANNEL_STATUS = "aura_status"

        /**
         * Weak reference to the Application context — safe for services
         * and workers that outlive Activities.
         */
        @Volatile
        @JvmStatic
        var appContext: Application? = null
            private set
    }

    override fun onCreate() {
        super.onCreate()
        appContext = this

        Log.i(TAG, "AuraApplication.onCreate()")

        createNotificationChannels()
        configureStrictMode()
    }

    // ── Notification Channels ───────────────────────────────────────────

    private fun createNotificationChannels() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return

        val nm = getSystemService(NotificationManager::class.java) ?: return

        // Foreground service channel — low importance, silent.
        val foreground = NotificationChannel(
            CHANNEL_FOREGROUND,
            "AURA Service",
            NotificationManager.IMPORTANCE_LOW
        ).apply {
            description = "Keeps the AURA daemon alive in the background"
            setShowBadge(false)
        }
        nm.createNotificationChannel(foreground)

        // Status / alert channel — default importance.
        val status = NotificationChannel(
            CHANNEL_STATUS,
            "AURA Status",
            NotificationManager.IMPORTANCE_DEFAULT
        ).apply {
            description = "Notifications about daemon health and task progress"
        }
        nm.createNotificationChannel(status)

        Log.d(TAG, "Notification channels created")
    }

    // ── Debug Helpers ───────────────────────────────────────────────────

    private fun configureStrictMode() {
        if (!BuildConfig.DEBUG) return

        StrictMode.setThreadPolicy(
            StrictMode.ThreadPolicy.Builder()
                .detectAll()
                .penaltyLog()
                .build()
        )
        StrictMode.setVmPolicy(
            StrictMode.VmPolicy.Builder()
                .detectLeakedSqlLiteObjects()
                .detectLeakedClosableObjects()
                .penaltyLog()
                .build()
        )
    }
}
