package social.coracle.dipity

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import androidx.core.content.ContextCompat
import androidx.core.content.FileProvider
import java.io.File

/** The private broadcast the chooser reports a chosen app on. */
private const val TAKEN = "social.coracle.dipity.BACKUP_TAKEN"

/**
 * The chooser a key backup is offered through, and the one honest answer it
 * gives.
 *
 * The activity result says only that the chooser is gone, because Android
 * returns `RESULT_CANCELED` from a share chooser whether or not an app took the
 * file. `docs/keys.md#backup` gates the flow on the download having happened. A
 * `shared` read off the result code would be false forever, and the user could
 * never get past the screen. The chooser's own report of what was picked is the
 * signal, and [taken] is it.
 *
 * The file reaches the other app as a `content://` uri through the FileProvider
 * the manifest declares over the cache directory. Nothing hands out a path.
 */
class Backup(private val context: Context) {
    private var chosen = false

    private val receiver =
        object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                chosen = true
            }
        }

    /** Whether an app took the last backup offered. */
    val taken: Boolean
        get() = chosen

    init {
        ContextCompat.registerReceiver(
            context,
            receiver,
            IntentFilter(TAKEN),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
    }

    fun stop() = context.unregisterReceiver(receiver)

    /**
     * The chooser for `file`, ready to start.
     *
     * The reporting intent is mutable because the system is what fills the
     * chosen component into it.
     */
    fun chooser(file: File): Intent {
        chosen = false

        val send =
            Intent(Intent.ACTION_SEND)
                .setType("text/plain")
                .putExtra(
                    Intent.EXTRA_STREAM,
                    FileProvider.getUriForFile(
                        context,
                        "${context.packageName}.fileprovider",
                        file,
                    ),
                )
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)

        val report =
            PendingIntent.getBroadcast(
                context,
                0,
                Intent(TAKEN).setPackage(context.packageName),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE,
            )

        return Intent.createChooser(send, null, report.intentSender)
    }
}
