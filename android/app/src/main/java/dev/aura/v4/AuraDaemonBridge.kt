package dev.aura.v4

import android.accessibilityservice.AccessibilityService
import android.app.ActivityManager
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager as AndroidSensorManager
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.wifi.WifiManager
import android.os.BatteryManager
import android.os.Build
import android.os.PowerManager
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import java.lang.ref.WeakReference

/**
 * Kotlin ↔ Rust JNI bridge singleton for AURA v4.
 *
 * ## Two-way bridge
 *
 * **Kotlin → Rust** (`external fun`):
 * Called from [AuraForegroundService] to initialise and run the daemon.
 *
 * **Rust → Kotlin** (`@JvmStatic`):
 * Called by the Rust `jni_bridge.rs` module via JNI `CallStaticMethod`.
 * Every `@JvmStatic` method below matches a corresponding Rust helper
 * function in `crate::platform::jni_bridge`.
 *
 * ## Thread safety
 * All `@JvmStatic` methods are safe to call from any thread.
 * The accessibility service reference is held as a [WeakReference] to
 * prevent leaks.
 */
object AuraDaemonBridge {

    private const val TAG = "AuraDaemonBridge"

    init {
        System.loadLibrary("aura_daemon")
        Log.i(TAG, "libaura_daemon.so loaded")
    }

    // ── Accessibility Service Reference ─────────────────────────────────

    @Volatile
    private var serviceRef: WeakReference<AuraAccessibilityService>? = null

    /** Called by [AuraAccessibilityService.onServiceConnected]. */
    @JvmStatic
    fun registerService(service: AuraAccessibilityService) {
        serviceRef = WeakReference(service)
        Log.i(TAG, "AccessibilityService registered")
    }

    /** Called by [AuraAccessibilityService.onDestroy]. */
    @JvmStatic
    fun unregisterService() {
        serviceRef = null
        Log.i(TAG, "AccessibilityService unregistered")
    }

    private fun service(): AuraAccessibilityService? = serviceRef?.get()

    private fun context(): Context? =
        service() ?: AuraApplication.appContext

    // ════════════════════════════════════════════════════════════════════
    //  EXTERNAL (Kotlin → Rust) — called from AuraForegroundService
    // ════════════════════════════════════════════════════════════════════

    /**
     * Initialise the Rust daemon.
     * @param configJson JSON string parsed into `AuraConfig` on the Rust side.
     * @return Opaque pointer to `DaemonState`, or 0 on failure.
     */
    @JvmStatic
    external fun nativeInit(configJson: String): Long

    /**
     * Enter the Rust main event loop (blocking).
     * @param statePtr Pointer returned by [nativeInit].
     */
    @JvmStatic
    external fun nativeRun(statePtr: Long)

    /** Request graceful shutdown of the Rust daemon. */
    @JvmStatic
    external fun nativeShutdown()

    // ════════════════════════════════════════════════════════════════════
    //  STATIC METHODS (Rust → Kotlin) — called via JNI CallStaticMethod
    //  Method names & signatures MUST match jni_bridge.rs exactly.
    // ════════════════════════════════════════════════════════════════════

    // ── Screen / Actions ────────────────────────────────────────────────

    /** Dispatch a tap gesture at screen coordinates (x, y). */
    @JvmStatic
    fun performTap(x: Int, y: Int): Boolean {
        val svc = service() ?: run {
            Log.w(TAG, "performTap: service unavailable")
            return false
        }
        return svc.dispatchTap(x, y)
    }

    /** Dispatch a swipe gesture from (x1,y1) to (x2,y2). */
    @JvmStatic
    fun performSwipe(x1: Int, y1: Int, x2: Int, y2: Int, durationMs: Int): Boolean {
        val svc = service() ?: run {
            Log.w(TAG, "performSwipe: service unavailable")
            return false
        }
        return svc.dispatchSwipe(x1, y1, x2, y2, durationMs.toLong())
    }

    /** Type text into the currently focused input. */
    @JvmStatic
    fun typeText(text: String): Boolean {
        val svc = service() ?: run {
            Log.w(TAG, "typeText: service unavailable")
            return false
        }
        return svc.dispatchTypeText(text)
    }

    /**
     * Capture the current accessibility tree as a bincode-encoded byte array
     * of `Vec<RawA11yNode>`.
     *
     * The Rust side deserialises this with `bincode::deserialize`.
     */
    @JvmStatic
    fun getScreenTree(): ByteArray {
        val svc = service() ?: run {
            Log.w(TAG, "getScreenTree: service unavailable")
            return ByteArray(0)
        }
        return svc.serializeScreenTree()
    }

    /** Press the global Back button. */
    @JvmStatic
    fun pressBack(): Boolean {
        val svc = service() ?: return false
        return svc.performGlobalAction(AccessibilityService.GLOBAL_ACTION_BACK)
    }

    /** Press the global Home button. */
    @JvmStatic
    fun pressHome(): Boolean {
        val svc = service() ?: return false
        return svc.performGlobalAction(AccessibilityService.GLOBAL_ACTION_HOME)
    }

    /** Press the global Recents button. */
    @JvmStatic
    fun pressRecents(): Boolean {
        val svc = service() ?: return false
        return svc.performGlobalAction(AccessibilityService.GLOBAL_ACTION_RECENTS)
    }

    /** Open the notification shade. */
    @JvmStatic
    fun openNotifications(): Boolean {
        val svc = service() ?: return false
        return svc.performGlobalAction(AccessibilityService.GLOBAL_ACTION_NOTIFICATIONS)
    }

    /** Return the package name of the current foreground app. */
    @JvmStatic
    fun getForegroundPackage(): String {
        val svc = service() ?: return ""
        return svc.currentPackageName ?: ""
    }

    /** Health-check: is the accessibility service connected? */
    @JvmStatic
    fun isServiceAlive(): Boolean = service() != null

    // ── Power / Battery ─────────────────────────────────────────────────

    /** Battery charge level 0-100. */
    @JvmStatic
    fun getBatteryLevel(): Int {
        val ctx = context() ?: return 50
        val bm = ctx.getSystemService(Context.BATTERY_SERVICE) as? BatteryManager
            ?: return fallbackBatteryLevel(ctx)
        return bm.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY)
    }

    /** Is the device currently charging? */
    @JvmStatic
    fun isCharging(): Boolean {
        val ctx = context() ?: return false
        val bm = ctx.getSystemService(Context.BATTERY_SERVICE) as? BatteryManager
            ?: return false
        return bm.isCharging
    }

    /** Is the app whitelisted from battery optimizations? */
    @JvmStatic
    fun isIgnoringBatteryOptimizations(): Boolean {
        val ctx = context() ?: return false
        val pm = ctx.getSystemService(Context.POWER_SERVICE) as? PowerManager ?: return false
        return pm.isIgnoringBatteryOptimizations(ctx.packageName)
    }

    // ── Thermal ─────────────────────────────────────────────────────────

    /**
     * Device temperature in degrees Celsius.
     *
     * Uses `ACTION_BATTERY_CHANGED` intent as a rough proxy; the actual
     * thermal framework (`PowerManager.THERMAL_STATUS_*`) requires API 29+
     * and isn't always reliable on OEM ROMs.
     */
    @JvmStatic
    fun getThermalStatus(): Float {
        val ctx = context() ?: return 35.0f
        return try {
            val intent = ctx.registerReceiver(
                null,
                IntentFilter(Intent.ACTION_BATTERY_CHANGED)
            )
            val temp = intent?.getIntExtra(BatteryManager.EXTRA_TEMPERATURE, 350) ?: 350
            temp / 10.0f  // tenths of a degree → degrees
        } catch (e: Exception) {
            Log.w(TAG, "getThermalStatus failed: ${e.message}")
            35.0f
        }
    }

    // ── Doze / Wakelock ─────────────────────────────────────────────────

    /** Is the device in Doze (idle) mode? */
    @JvmStatic
    fun isDozeMode(): Boolean {
        val ctx = context() ?: return false
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return false
        val pm = ctx.getSystemService(Context.POWER_SERVICE) as? PowerManager ?: return false
        return pm.isDeviceIdleMode
    }

    @Volatile
    private var managedWakeLock: PowerManager.WakeLock? = null

    /** Acquire a partial wake-lock with the given tag and timeout. */
    @JvmStatic
    fun acquireWakelock(tag: String, timeoutMs: Long) {
        val ctx = context() ?: return
        val pm = ctx.getSystemService(Context.POWER_SERVICE) as? PowerManager ?: return

        // Release any existing lock first.
        releaseWakelock()

        managedWakeLock = pm.newWakeLock(
            PowerManager.PARTIAL_WAKE_LOCK,
            "aura:$tag"
        ).apply {
            acquire(timeoutMs)
        }
        Log.d(TAG, "WakeLock acquired: tag=$tag timeout=${timeoutMs}ms")
    }

    /** Release the previously acquired wake-lock. */
    @JvmStatic
    fun releaseWakelock() {
        managedWakeLock?.let {
            if (it.isHeld) {
                it.release()
                Log.d(TAG, "WakeLock released")
            }
        }
        managedWakeLock = null
    }

    // ── Notifications ───────────────────────────────────────────────────

    /** Register an Android notification channel (no-op below API 26). */
    @JvmStatic
    fun registerNotificationChannel(id: String, name: String, importance: Int) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val ctx = context() ?: return
        val nm = ctx.getSystemService(NotificationManager::class.java) ?: return
        val channel = NotificationChannel(id, name, importance)
        nm.createNotificationChannel(channel)
        Log.d(TAG, "Notification channel registered: $id")
    }

    /** Post a notification. */
    @JvmStatic
    fun postNotification(
        id: Int,
        channelId: String,
        title: String,
        body: String,
        ongoing: Boolean
    ) {
        val ctx = context() ?: return
        val notification = NotificationCompat.Builder(ctx, channelId)
            .setContentTitle(title)
            .setContentText(body)
            .setSmallIcon(android.R.drawable.ic_menu_info_details)
            .setOngoing(ongoing)
            .build()

        try {
            NotificationManagerCompat.from(ctx).notify(id, notification)
        } catch (e: SecurityException) {
            Log.w(TAG, "postNotification: missing POST_NOTIFICATIONS permission")
        }
    }

    /** Cancel a notification by its ID. */
    @JvmStatic
    fun cancelNotification(id: Int) {
        val ctx = context() ?: return
        NotificationManagerCompat.from(ctx).cancel(id)
    }

    // ── System Info ─────────────────────────────────────────────────────

    /** Available system memory in megabytes. */
    @JvmStatic
    fun getAvailableMemoryMb(): Long {
        val ctx = context() ?: return 512L
        val am = ctx.getSystemService(Context.ACTIVITY_SERVICE) as? ActivityManager
            ?: return 512L
        val memInfo = ActivityManager.MemoryInfo()
        am.getMemoryInfo(memInfo)
        return memInfo.availMem / (1024 * 1024)
    }

    /**
     * Generic action execution via JSON.
     *
     * Used as a fallback for action types that don't have dedicated JNI
     * bridge methods (OpenApp, NotificationAction, WaitForElement, etc.).
     */
    @JvmStatic
    fun executeAction(actionJson: String): Boolean {
        val svc = service() ?: run {
            Log.w(TAG, "executeAction: service unavailable")
            return false
        }
        return svc.executeGenericAction(actionJson)
    }

    // ── Sensors ───────────────────────────────────────────────────────

    /** Cached sensor readings — updated by a background listener. */
    @Volatile private var lastAccelX = 0.0f
    @Volatile private var lastAccelY = 0.0f
    @Volatile private var lastAccelZ = 9.81f
    @Volatile private var lastLightLux = 100.0f
    @Volatile private var lastProximityNear = false
    @Volatile private var lastStepCount = 0
    @Volatile private var sensorListenerRegistered = false

    /** Start listening to sensors. Called once at daemon init. */
    @JvmStatic
    fun startSensorListeners() {
        val ctx = context() ?: return
        val sm = ctx.getSystemService(Context.SENSOR_SERVICE) as? AndroidSensorManager ?: return
        if (sensorListenerRegistered) return

        val listener = object : SensorEventListener {
            override fun onSensorChanged(event: SensorEvent) {
                when (event.sensor.type) {
                    Sensor.TYPE_ACCELEROMETER -> {
                        lastAccelX = event.values[0]
                        lastAccelY = event.values[1]
                        lastAccelZ = event.values[2]
                    }
                    Sensor.TYPE_LIGHT -> {
                        lastLightLux = event.values[0]
                    }
                    Sensor.TYPE_PROXIMITY -> {
                        lastProximityNear = event.values[0] < event.sensor.maximumRange
                    }
                    Sensor.TYPE_STEP_COUNTER -> {
                        lastStepCount = event.values[0].toInt()
                    }
                }
            }
            override fun onAccuracyChanged(sensor: Sensor, accuracy: Int) {}
        }

        listOf(
            Sensor.TYPE_ACCELEROMETER,
            Sensor.TYPE_LIGHT,
            Sensor.TYPE_PROXIMITY,
            Sensor.TYPE_STEP_COUNTER
        ).forEach { type ->
            sm.getDefaultSensor(type)?.let { sensor ->
                sm.registerListener(listener, sensor, AndroidSensorManager.SENSOR_DELAY_NORMAL)
            }
        }
        sensorListenerRegistered = true
        Log.i(TAG, "Sensor listeners registered")
    }

    /** Get latest accelerometer reading as float[3] (x, y, z in m/s²). */
    @JvmStatic
    fun getAccelerometer(): FloatArray = floatArrayOf(lastAccelX, lastAccelY, lastAccelZ)

    /** Get latest ambient light level in lux. */
    @JvmStatic
    fun getLightLevel(): Float = lastLightLux

    /** Is an object near the proximity sensor? */
    @JvmStatic
    fun isProximityNear(): Boolean = lastProximityNear

    /** Cumulative step count since boot. */
    @JvmStatic
    fun getStepCount(): Int = lastStepCount

    // ── Connectivity ────────────────────────────────────────────────────

    /** Get current network type: "wifi", "cellular", "ethernet", or "none". */
    @JvmStatic
    fun getNetworkType(): String {
        val ctx = context() ?: return "none"
        val cm = ctx.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager
            ?: return "none"
        val net = cm.activeNetwork ?: return "none"
        val caps = cm.getNetworkCapabilities(net) ?: return "none"
        return when {
            caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> "wifi"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "cellular"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> "ethernet"
            else -> "none"
        }
    }

    /** Is the active network metered? */
    @JvmStatic
    fun isNetworkMetered(): Boolean {
        val ctx = context() ?: return false
        val cm = ctx.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager
            ?: return false
        return cm.isActiveNetworkMetered
    }

    /** Get WiFi RSSI (signal strength) in dBm. Returns -100 if unavailable. */
    @JvmStatic
    fun getWifiRssi(): Int {
        val ctx = context() ?: return -100
        val wm = ctx.applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            ?: return -100
        @Suppress("DEPRECATION")
        return wm.connectionInfo?.rssi ?: -100
    }

    /** Is any network available? */
    @JvmStatic
    fun isNetworkAvailable(): Boolean {
        val ctx = context() ?: return false
        val cm = ctx.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager
            ?: return false
        val net = cm.activeNetwork ?: return false
        val caps = cm.getNetworkCapabilities(net) ?: return false
        return caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
    }

    // ── OEM Detection ───────────────────────────────────────────────────

    /** Get device manufacturer (e.g., "Xiaomi", "samsung", "HUAWEI"). */
    @JvmStatic
    fun getDeviceManufacturer(): String = Build.MANUFACTURER

    /**
     * Check if the app has autostart permission.
     *
     * This is a best-effort check — many OEMs don't expose this via API.
     * Returns true if we believe autostart is allowed (or can't determine).
     */
    @JvmStatic
    fun hasAutostartPermission(): Boolean {
        // Most OEMs don't expose a reliable API for this.
        // We check battery optimization as a proxy — if exempted,
        // the app is likely whitelisted.
        return isIgnoringBatteryOptimizations()
    }

    // ── Private Helpers ─────────────────────────────────────────────────

    private fun fallbackBatteryLevel(ctx: Context): Int {
        return try {
            val intent = ctx.registerReceiver(
                null,
                IntentFilter(Intent.ACTION_BATTERY_CHANGED)
            )
            val level = intent?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1) ?: -1
            val scale = intent?.getIntExtra(BatteryManager.EXTRA_SCALE, 100) ?: 100
            if (level >= 0 && scale > 0) (level * 100) / scale else 50
        } catch (_: Exception) {
            50
        }
    }
}
