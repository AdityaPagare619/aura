package dev.aura.v4

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Foreground service that hosts the Rust AURA daemon on a native thread.
 *
 * ## Lifecycle
 * 1. `onCreate`  — post the mandatory foreground notification.
 * 2. `onStartCommand` — spawn a daemon thread that calls `nativeInit` then
 *    `nativeRun` (blocking). The thread name is "aura-daemon" so it's easy
 *    to identify in profilers.
 * 3. `onDestroy` — call `nativeShutdown`, release the partial wake-lock,
 *    and join the daemon thread.
 *
 * ## OEM Kill Recovery
 * On aggressive OEM ROMs (Xiaomi, Samsung, Huawei) the service may still be
 * killed. Returning `START_STICKY` combined with the [BootReceiver] provides
 * best-effort restart.
 */
class AuraForegroundService : Service() {

    companion object {
        private const val TAG = "AuraForegroundSvc"
        private const val NOTIFICATION_ID = 1
        private const val WAKELOCK_TAG = "aura:daemon"
        private const val WAKELOCK_TIMEOUT_MS = 10L * 60 * 1000 // 10 min, renewed
    }

    private var daemonThread: Thread? = null
    private var wakeLock: PowerManager.WakeLock? = null
    private val isRunning = AtomicBoolean(false)

    // ── Service Lifecycle ───────────────────────────────────────────────

    override fun onCreate() {
        super.onCreate()
        Log.i(TAG, "onCreate")
        startForeground(NOTIFICATION_ID, buildNotification())
        acquireWakeLock()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        Log.i(TAG, "onStartCommand (flags=$flags, startId=$startId)")

        if (isRunning.compareAndSet(false, true)) {
            startDaemonThread()
        } else {
            Log.w(TAG, "Daemon thread already running — ignoring duplicate start")
        }

        // START_STICKY: system will restart the service after OOM kill.
        return START_STICKY
    }

    override fun onDestroy() {
        Log.i(TAG, "onDestroy — shutting down daemon")
        isRunning.set(false)

        // Signal the Rust side to stop.
        try {
            AuraDaemonBridge.nativeShutdown()
        } catch (e: Throwable) {
            Log.e(TAG, "nativeShutdown threw: ${e.message}")
        }

        // Wait for the daemon thread to exit (with a generous timeout).
        daemonThread?.let { t ->
            try {
                t.join(5_000)
                if (t.isAlive) {
                    Log.w(TAG, "Daemon thread did not exit within 5 s — interrupting")
                    t.interrupt()
                }
            } catch (_: InterruptedException) {
                Thread.currentThread().interrupt()
            }
        }
        daemonThread = null

        releaseWakeLock()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    // ── Daemon Thread ───────────────────────────────────────────────────

    private fun startDaemonThread() {
        daemonThread = Thread({
            try {
                Log.i(TAG, "Daemon thread started")

                // Pass minimal config JSON — the Rust side defaults for any
                // missing fields.
                val configJson = buildConfigJson()
                val statePtr = AuraDaemonBridge.nativeInit(configJson)

                if (statePtr == 0L) {
                    Log.e(TAG, "nativeInit returned null pointer — aborting")
                    stopSelf()
                    return@Thread
                }

                // This blocks until the main loop exits (shutdown or error).
                AuraDaemonBridge.nativeRun(statePtr)

                Log.i(TAG, "Daemon main loop exited normally")
            } catch (e: Throwable) {
                Log.e(TAG, "Daemon thread crashed: ${e.message}", e)
            } finally {
                isRunning.set(false)
            }
        }, "aura-daemon").also { it.start() }
    }

    private fun buildConfigJson(): String {
        // Minimal JSON — Rust AuraConfig::default() handles everything else.
        return """{"platform":"android"}"""
    }

    // ── Foreground Notification ─────────────────────────────────────────

    private fun buildNotification(): Notification {
        val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Notification.Builder(this, AuraApplication.CHANNEL_FOREGROUND)
        } else {
            @Suppress("DEPRECATION")
            Notification.Builder(this)
        }

        return builder
            .setContentTitle("AURA is running")
            .setContentText("Autonomous agent active")
            .setSmallIcon(android.R.drawable.ic_menu_manage)
            .setOngoing(true)
            .setCategory(Notification.CATEGORY_SERVICE)
            .build()
    }

    // ── WakeLock Management ─────────────────────────────────────────────

    private fun acquireWakeLock() {
        val pm = getSystemService(POWER_SERVICE) as? PowerManager ?: return
        wakeLock = pm.newWakeLock(
            PowerManager.PARTIAL_WAKE_LOCK,
            WAKELOCK_TAG
        ).apply {
            acquire(WAKELOCK_TIMEOUT_MS)
        }
        Log.d(TAG, "WakeLock acquired (timeout=${WAKELOCK_TIMEOUT_MS}ms)")
    }

    private fun releaseWakeLock() {
        wakeLock?.let {
            if (it.isHeld) {
                it.release()
                Log.d(TAG, "WakeLock released")
            }
        }
        wakeLock = null
    }
}
