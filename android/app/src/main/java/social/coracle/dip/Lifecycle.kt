package social.coracle.dip

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.BatteryManager
import androidx.core.content.ContextCompat

/** The private broadcast an armed [Lifecycle.wake] comes back on. */
private const val WAKE = "social.coracle.dip.WAKE"

/**
 * What the battery is at, and when the core wants ticking.
 *
 * Both are things the core asks for and nothing answered: blob transfers are
 * metered against `BLOB_MIN_BATTERY`, so with no report the core is deciding on
 * missing information, and a `WakeAt` nobody acts on leaves a session holding
 * one of six link slots until the radio happens to fire.
 *
 * Where the app is stays with the plugin, which is where Android surfaces it —
 * `handleOnResume` and `handleOnPause` are the activity's, not this object's.
 *
 * ## The alarm
 *
 * Inexact and allowed while idle. Exact alarms need `SCHEDULE_EXACT_ALARM`, a
 * permission the user grants in Settings, and `WakeAt` is advisory — a tick a
 * few minutes late expires a heartbeat late, where a tick that never comes at
 * all is a slot held forever. The broadcast is delivered to a receiver
 * registered here rather than in the manifest, which is sound because
 * [EncounterService] is what keeps the process alive with the screen off.
 */
class Lifecycle(
    private val context: Context,
    private val battery: (UByte) -> Unit,
    private val tick: () -> Unit,
) {
    private val alarms = context.getSystemService(AlarmManager::class.java)

    private val alarm =
        PendingIntent.getBroadcast(
            context,
            0,
            Intent(WAKE).setPackage(context.packageName),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

    private val receiver =
        object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                when (intent.action) {
                    Intent.ACTION_BATTERY_CHANGED -> report(intent)
                    WAKE -> tick()
                }
            }
        }

    /**
     * Listen, and report the battery as it is now.
     *
     * `ACTION_BATTERY_CHANGED` is sticky, so registering for it answers the
     * current level: a device that is launched and left alone is not waiting on
     * a change before the core knows anything.
     */
    fun start() {
        val filter =
            IntentFilter(Intent.ACTION_BATTERY_CHANGED).apply { addAction(WAKE) }

        ContextCompat.registerReceiver(
                context,
                receiver,
                filter,
                ContextCompat.RECEIVER_NOT_EXPORTED,
            )
            ?.let(::report)
    }

    fun stop() {
        alarms.cancel(alarm)
        context.unregisterReceiver(receiver)
    }

    /** Tick at or after `at`. */
    fun wake(at: Long) {
        alarms.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at * 1000, alarm)
    }

    /**
     * The level as a percentage.
     *
     * A scale of zero is an emulator with no battery to report, and is not
     * worth reporting as a flat one.
     */
    private fun report(status: Intent) {
        val level = status.getIntExtra(BatteryManager.EXTRA_LEVEL, -1)
        val scale = status.getIntExtra(BatteryManager.EXTRA_SCALE, -1)

        if (level < 0 || scale <= 0) return

        battery((level * 100 / scale).toUByte())
    }
}
