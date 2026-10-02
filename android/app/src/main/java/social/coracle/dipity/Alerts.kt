package social.coracle.dipity

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import uniffi.dip_ffi.Announcement

/**
  * Local notifications. The core decides what to say and when, and the shell
 * posts it.
 *
 * Each announcement has one id, so a newer count replaces the last rather than
 * stacking. `docs/storage.md#notifications`.
 */
object Alerts {
    private const val CHANNEL = "social.coracle.dipity.alerts"
    private const val PAIRING = 2
    private const val CONTENT = 3

    /** Post what the core asked for, if the user has let the app notify. */
    fun post(context: Context, announcement: Announcement) {
        val manager = NotificationManagerCompat.from(context)

        if (!manager.areNotificationsEnabled()) return

        channel(context)

        val (id, title, text) =
            when (announcement) {
                is Announcement.Pairing ->
                    Triple(PAIRING, "Somebody nearby wants to pair", "Open Dipity to compare shapes with them.")
                is Announcement.Content -> {
                    val count = announcement.count.toInt()
                    val title = if (count == 1) "New post on the board" else "$count new posts on the board"

                    Triple(CONTENT, title, "Open Dipity to read.")
                }
            }

        val launch = context.packageManager.getLaunchIntentForPackage(context.packageName)
        val open =
            launch?.let {
                PendingIntent.getActivity(context, id, it, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            }

        val notification =
            NotificationCompat.Builder(context, CHANNEL)
                .setSmallIcon(R.drawable.ic_stat_dip)
                .setContentTitle(title)
                .setContentText(text)
                .setContentIntent(open)
                .setAutoCancel(true)
                .build()

        try {
            manager.notify(id, notification)
        } catch (error: SecurityException) {
            android.util.Log.w("dip", "a notification was refused: $error")
        }
    }

    /** The user is looking, so what was announced has been seen. */
    fun clear(context: Context) {
        val manager = NotificationManagerCompat.from(context)

        manager.cancel(PAIRING)
        manager.cancel(CONTENT)
    }

    /** Whether notifications are on at all, which is the whole answer before Android 13. */
    fun enabled(context: Context) = NotificationManagerCompat.from(context).areNotificationsEnabled()

    private fun channel(context: Context) {
        context
            .getSystemService(NotificationManager::class.java)
            .createNotificationChannel(
                NotificationChannel(CHANNEL, "Pairing and new posts", NotificationManager.IMPORTANCE_DEFAULT)
            )
    }
}
