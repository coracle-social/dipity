package social.coracle.dip

import android.content.Context
import android.content.pm.ApplicationInfo
import android.util.Log
import uniffi.dip_ffi.LogLevel
import uniffi.dip_ffi.Logger
import uniffi.dip_ffi.initLogging

/**
 * The core's `log` records, on logcat.
 *
 * The Rust module path arrives as the record's target and becomes the tag, so
 * `dip::sync::blob` filters on its own without the core naming a subsystem it
 * cannot see. Records come in on whichever thread logged them, which logcat
 * takes from any.
 */
class Logcat : Logger {
    override fun log(level: LogLevel, target: String, message: String) {
        when (level) {
            LogLevel.ERROR -> Log.e(target, message)
            LogLevel.WARN -> Log.w(target, message)
            LogLevel.INFO -> Log.i(target, message)
            LogLevel.DEBUG -> Log.d(target, message)
            LogLevel.TRACE -> Log.v(target, message)
        }
    }

    companion object {
        /**
         * Route the core's log here, at the level this build carries.
         *
         * Called once, before the core is opened, so a failure on the way up is
         * logged rather than being the first thing nobody sees. Debuggable is
         * read off the manifest rather than `BuildConfig`, which the app module
         * does not generate.
         */
        fun install(context: Context) {
            val debuggable = context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0

            initLogging(Logcat(), if (debuggable) LogLevel.DEBUG else LogLevel.INFO)
        }
    }
}
