package dev.aura.v4

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
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
        // AND-CRIT-004: WakeLock timeout with renewal. The Handler renews the
        // lock every 9 minutes so it never expires while the daemon is running.
        // Using a finite timeout (not indefinite) is Android best practice —
        // it ensures the lock auto-releases if the renewal mechanism itself dies.
        private const val WAKELOCK_TIMEOUT_MS = 10L * 60 * 1000 // 10 min
        private const val WAKELOCK_RENEW_INTERVAL_MS = 9L * 60 * 1000 // 9 min
    }

    private var daemonThread: Thread? = null
    private var wakeLock: PowerManager.WakeLock? = null
    private val isRunning = AtomicBoolean(false)
    // AND-CRIT-004: Handler for periodic WakeLock renewal.
    private val renewHandler = Handler(Looper.getMainLooper())

    // ── Service Lifecycle ───────────────────────────────────────────────

    override fun onCreate() {
        super.onCreate()
        Log.i(TAG, "onCreate")
        // AND-CRIT-001: On API 34+ (Android 14), startForeground() MUST include
        // the foregroundServiceType bitmask. Without this, the system throws
        // MissingForegroundServiceTypeException and kills the service.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(
                NOTIFICATION_ID,
                buildNotification(),
                ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
            )
        } else {
            startForeground(NOTIFICATION_ID, buildNotification())
        }
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

        // AND-CRIT-004: Stop WakeLock renewal before releasing.
        renewHandler.removeCallbacksAndMessages(null)

        // AND-HIGH-6: nativeShutdown() may block on Rust-side channel drain.
        // Dispatching to a background thread prevents ANR if onDestroy() is
        // called on the main thread (which it always is).
        val shutdownThread = Thread({
            try {
                AuraDaemonBridge.nativeShutdown()
            } catch (e: Throwable) {
                Log.e(TAG, "nativeShutdown threw: ${e.message}")
            }
        }, "aura-shutdown")
        shutdownThread.start()

        // Wait for nativeShutdown to complete (bounded).
        try {
            shutdownThread.join(2_000)
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
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

        // AND-CRIT-004: Schedule periodic renewal. The lock has a 10-min timeout;
        // we renew every 9 min so it never expires while the service is alive.
        // If the service dies, the Handler is GC'd and the lock auto-expires —
        // this is the safe-by-design pattern recommended by Android docs.
        renewHandler.postDelayed(object : Runnable {
            override fun run() {
                wakeLock?.let { wl ->
                    if (wl.isHeld) {
                        wl.acquire(WAKELOCK_TIMEOUT_MS)
                        Log.d(TAG, "WakeLock renewed (timeout=${WAKELOCK_TIMEOUT_MS}ms)")
                    }
                }
                renewHandler.postDelayed(this, WAKELOCK_RENEW_INTERVAL_MS)
            }
        }, WAKELOCK_RENEW_INTERVAL_MS)
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
