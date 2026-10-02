package social.coracle.dipity

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder

/**
 * What keeps the radio alive with the screen off.
 *
 * The whole premise is gossip continuing while both phones are in pockets, and
 * Android will not scan indefinitely from a backgrounded activity — a scan
 * started by one is throttled hard and then stopped. This service is the only
 * thing here that is not a mirror of the iOS side, where the equivalent is two
 * background modes and state restoration.
 *
 * It holds no state and drives nothing. The radio and the node live in
 * [Encounters], which outlives the activity; this is the foreground declaration
 * that lets them keep working, and what reopens them when the system restarts it.
 */
class EncounterService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val notification =
            Notification.Builder(this, CHANNEL)
                .setContentTitle(getString(R.string.encounter_title))
                .setContentText(getString(R.string.encounter_text))
                .setSmallIcon(android.R.drawable.stat_sys_data_bluetooth)
                .setOngoing(true)
                .build()

        startForeground(
            NOTIFICATION,
            notification,
            ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE,
        )

        // A restart after the system killed the process has nothing open, and nobody else will open it.
        Encounters.revive(this)

        // Restarted without its intent if the system kills it: there is nothing
        // in the intent, and stopping means never meeting anyone again.
        return START_STICKY
    }

    companion object {
        private const val CHANNEL = "social.coracle.dipity.encounters"
        private const val NOTIFICATION = 1

        /** Start it, creating its notification channel the first time. */
        fun start(context: Context) {
            val manager = context.getSystemService(NotificationManager::class.java)

            manager.createNotificationChannel(
                NotificationChannel(
                        CHANNEL,
                        context.getString(R.string.encounter_channel),
                        NotificationManager.IMPORTANCE_LOW,
                    )
                    .apply { setShowBadge(false) }
            )

            context.startForegroundService(Intent(context, EncounterService::class.java))
        }

        /** Stop it, which is the user turning the app off rather than leaving it. */
        fun stop(context: Context) {
            context.stopService(Intent(context, EncounterService::class.java))
        }
    }
}
