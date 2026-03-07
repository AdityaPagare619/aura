package dev.aura.v4

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Log

/**
 * Starts [AuraForegroundService] after device boot so the daemon
 * survives reboots without manual app launch.
 *
 * Registered in the manifest for `BOOT_COMPLETED` and
 * `QUICKBOOT_POWERON` (HTC / some OEMs).
 */
class BootReceiver : BroadcastReceiver() {

    companion object {
        private const val TAG = "AuraBootReceiver"
    }

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED &&
            intent.action != "android.intent.action.QUICKBOOT_POWERON"
        ) {
            return
        }

        Log.i(TAG, "Boot completed — starting AuraForegroundService")

        val svcIntent = Intent(context, AuraForegroundService::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            context.startForegroundService(svcIntent)
        } else {
            context.startService(svcIntent)
        }
    }
}
