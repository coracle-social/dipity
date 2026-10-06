package social.coracle.dipity

import android.Manifest
import android.os.Handler
import android.os.Looper
import androidx.activity.OnBackPressedCallback
import androidx.activity.result.ActivityResult
import com.getcapacitor.JSObject
import com.getcapacitor.Plugin
import com.getcapacitor.PluginCall
import com.getcapacitor.PluginMethod
import com.getcapacitor.annotation.ActivityCallback
import com.getcapacitor.annotation.CapacitorPlugin
import com.getcapacitor.annotation.Permission
import com.getcapacitor.annotation.PermissionCallback
import com.getcapacitor.JSArray
import java.io.File
import org.json.JSONObject
import uniffi.dip_ffi.Action
import uniffi.dip_ffi.Change
import uniffi.dip_ffi.LinkId
import uniffi.dip_ffi.Order
import uniffi.dip_ffi.Query
import uniffi.dip_ffi.Store
import uniffi.dip_ffi.changeName
import uniffi.dip_ffi.coreVersion as loadedCoreVersion
import uniffi.dip_ffi.generateIdentity
import uniffi.dip_ffi.identityFromNsec
import uniffi.dip_ffi.identityNpub
import uniffi.dip_ffi.mediaTags as coreMediaTags

/** The three permissions the radio needs on 31+, asked for together. */
private const val RADIO_PERMISSIONS = "radio"

private const val NOTIFICATION_PERMISSION = "notifications"

/** How long a shared backup stays on disk for the app it went to, in milliseconds. */
private const val SHARE_GRACE = 5 * 60 * 1000L

/**
 * The webview's end of the core.
 *
 * A method per call the view makes, over the core [Encounters] holds for as long
 * as the process lives. The plugin lives only as long as its activity, so it
 * opens nothing it would have to close: it attaches to [Encounters] as the view,
 * and detaches when the activity goes. Everything below the bridge is
 * `dip_ffi`; nothing here decides anything about gossip, the radio schedule or
 * policy.
 *
 * ## The three call shapes
 *
 * A store read answers what it read, through [answer]. A node call answers only
 * the actions it asked for, through [perform]. A radio event has nobody waiting
 * on it, through [Encounters.drive]. Each takes the opened core, runs one entry point and
 * carries out the result; a method that spells any of that out again is a method
 * doing something the other two are not.
 *
 * ## The permissions
 *
 * Asked for before the radio starts, never after. A scan that silently returns
 * nothing because a permission was never granted looks exactly like a place with
 * nobody in it, which is the one failure this design cannot see from the inside.
 */
@CapacitorPlugin(
    name = "Dip",
    permissions =
        [
            Permission(
                alias = RADIO_PERMISSIONS,
                strings =
                    [
                        Manifest.permission.BLUETOOTH_SCAN,
                        Manifest.permission.BLUETOOTH_ADVERTISE,
                        Manifest.permission.BLUETOOTH_CONNECT,
                    ],
            ),
            Permission(alias = NOTIFICATION_PERMISSION, strings = [Manifest.permission.POST_NOTIFICATIONS]),
        ],
)
class DipPlugin : Plugin(), Encounters.View {
    private val keystore by lazy { Keystore(context) }

    /** The chooser a backup is offered through, which needs the activity. */
    private var backup: Backup? = null

    /** The `exportKey` call waiting on the chooser it opened. */
    private var exporting: PluginCall? = null

    /** The core, whoever opened it. */
    private val core: Encounters.Core?
        get() = Encounters.core

    /** Attach as the view, and claim the back button before anything draws. */
    override fun load() {
        backup = Backup(context)
        Encounters.attach(this)
        activity.onBackPressedDispatcher.addCallback(activity, back)
    }

    // ------------------------------------------------------------- Identity

    @PluginMethod
    fun coreVersion(call: PluginCall) {
        call.resolve(JSObject().put("version", loadedCoreVersion()))
    }

    @PluginMethod
    fun hasIdentity(call: PluginCall) {
        call.resolve(JSObject().put("exists", keystore.has()))
    }

    @PluginMethod
    fun createIdentity(call: PluginCall) {
        adopt(generateIdentity(), call)
    }

    @PluginMethod
    fun importIdentity(call: PluginCall) {
        val nsec = call.getString("nsec") ?: return call.reject("importIdentity needs an nsec")

        try {
            adopt(identityFromNsec(nsec), call)
        } catch (error: Exception) {
            call.reject("that is not an nsec", error)
        }
    }

    /** Forget the identity, closing the core that runs as it. */
    @PluginMethod
    fun deleteIdentity(call: PluginCall) {
        Encounters.close()
        call.resolve(JSObject().put("existed", keystore.delete()))
    }

    // ----------------------------------------------------------------- Node

    /**
     * Open the store and the node, and start the radio, unless the service
     * already has.
     *
     * The view calls this once it knows there is an identity, which is what
     * makes first run a screen rather than a failed open.
     */
    @PluginMethod
    fun start(call: PluginCall) {
        // The service may have opened it already, and a reloaded webview starts again over it.
        core?.let {
            Encounters.foreground(true)
            return call.resolve(JSObject().put("identity", it.node.identity()))
        }

        if (getPermissionState(RADIO_PERMISSIONS) != com.getcapacitor.PermissionState.GRANTED) {
            return requestPermissionForAlias(RADIO_PERMISSIONS, call, "radioGranted")
        }

        open(call)
    }

    @PermissionCallback
    fun radioGranted(call: PluginCall) {
        if (getPermissionState(RADIO_PERMISSIONS) != com.getcapacitor.PermissionState.GRANTED) {
            return call.reject("dip cannot meet anyone without the Bluetooth permissions")
        }

        open(call)
    }

    /**
     * The `imeta` entries an event has to carry for a peer to fetch `media` and
     * check what it gets, base64 in.
     *
     * Describing bytes stores nothing, so this is the one node call needing no
     * started core: the view composes the tag, signs the event, and hands both
     * to [publish]. `docs/storage.md#blob-store`.
     */
    @PluginMethod
    fun mediaTags(call: PluginCall) {
        val media = call.getString("media") ?: return call.reject("mediaTags needs base64 media")

        val bytes =
            try {
                android.util.Base64.decode(media, android.util.Base64.DEFAULT)
            } catch (error: IllegalArgumentException) {
                return call.reject("mediaTags needs base64 media", error)
            }

        call.resolve(JSObject().put("entries", JSArray(coreMediaTags(bytes))))
    }

    /** Store and offer an event the view built, with the media it attaches. */
    @PluginMethod
    fun publish(call: PluginCall) {
        val event = call.getString("event") ?: return call.reject("publish needs an event")
        val media =
            try {
                call.getArray("media")?.toList<String>().orEmpty().map {
                    android.util.Base64.decode(it, android.util.Base64.DEFAULT)
                }
            } catch (error: IllegalArgumentException) {
                return call.reject("publish needs base64 media", error)
            }

        perform(call, "that event could not be published") { it.node.publish(event, media) }
    }

    /** The user answered a `requestApproval` the plugin sent up. */
    @PluginMethod
    fun approve(call: PluginCall) {
        val link = link(call) ?: return call.reject("approve needs a link")

        perform(call, "that approval could not be recorded") {
            it.node.approve(link, call.getBoolean("approved", false) == true)
        }
    }

    // ------------------------------------------------- Login with device

    /**
     * Offer this device's identity to the peer on `link`.
     *
     * Both ends are asked to compare the six digits a `confirmIdentityTransfer`
     * carries before anything moves. `docs/keys.md#login-with-device`.
     */
    @PluginMethod
    fun offerIdentity(call: PluginCall) {
        val link = link(call) ?: return call.reject("offerIdentity needs a link")

        OwnerCheck.confirm(activity, "Put your key on another phone", call) {
            perform(call, "that identity could not be offered") { it.node.offerIdentity(link) }
        }
    }

    /** The user answered a `confirmIdentityTransfer` the plugin sent up. */
    @PluginMethod
    fun answerIdentityTransfer(call: PluginCall) {
        val link = link(call) ?: return call.reject("answerIdentityTransfer needs a link")

        perform(call, "that answer could not be recorded") {
            it.node.answerIdentityTransfer(link, call.getBoolean("confirmed", false) == true)
        }
    }

    /**
     * Adopt the identity an `identityTransfer` of `received` announced.
     *
     * The key is handed out once and does not cross the bridge: it is written to
     * the Keystore here, the same custody path a generated one takes, and the
     * node is reopened under it. Answers what `start` answers, so the view reads
     * the new identity off the same field.
     */
    @PluginMethod
    fun takeTransferredIdentity(call: PluginCall) {
        val core = this.core ?: return call.reject("takeTransferredIdentity needs a started core")
        val link = link(call) ?: return call.reject("takeTransferredIdentity needs a link")

        val secret =
            try {
                core.node.takeTransferredIdentity(link)
                    ?: return call.reject("no identity arrived on that link")
            } catch (error: Exception) {
                return call.reject("the transferred identity could not be adopted", error)
            }

        try {
            keystore.write(secret)

            // What the first-run identity gathered was its own, and goes with it.
            core.store.wipe()
        } catch (error: Exception) {
            return call.reject("the transferred identity could not be stored", error)
        } finally {
            secret.fill(0)
        }

        Encounters.close()
        open(call)

        for (group in listOf(Change.EVENTS, Change.BLOBS, Change.PREFERENCES)) {
            notify("storeChanged", JSObject().put("group", changeName(group)))
        }
    }

    /**
     * Write a key backup into the cache directory and offer it to the chooser.
     *
     * The path never comes back over the bridge: the view learns only that the
     * file was shared or dismissed. `docs/keys.md#backup`.
     */
    @PluginMethod
    fun exportKey(call: PluginCall) {
        OwnerCheck.confirm(activity, "Save a copy of your key", call) { export(call) }
    }

    private fun export(call: PluginCall) {
        val core = this.core ?: return call.reject("exportKey needs a started core")

        // One chooser means one call waiting on it, so whoever this displaces
        // is answered rather than left on a promise that never settles.
        exporting?.reject("another key export replaced this one")
        exporting = call

        try {
            Encounters.apply(
                core.node.exportKey(context.cacheDir.absolutePath, call.getString("password"))
            )
        } catch (error: Exception) {
            exporting = null
            call.reject("the backup could not be written", error)
        }
    }

    /**
     * The chooser closed, taken or dismissed, so the file goes.
     *
     * `result` says only that the chooser is gone: Android answers
     * `RESULT_CANCELED` whether or not an app took the file. What the view is
     * told comes from [Backup], which is the chooser's own report of what was
     * picked.
     */
    @ActivityCallback
    fun keyBackupClosed(call: PluginCall, result: ActivityResult) {
        notifyListeners("keyBackupShared", JSObject().put("shared", backup?.taken == true))

        // The chooser returns once an app is picked, which may read the file after; it goes later.
        Handler(Looper.getMainLooper())
            .postDelayed({ Encounters.drive { it.keyExportFinished() } }, SHARE_GRACE)
        call.resolve()
    }

    override fun handleOnResume() = Encounters.foreground(true)

    override fun handleOnPause() = Encounters.foreground(false)

    /** The activity is going, and the core stays: gossip carries on without a screen. */
    override fun handleOnDestroy() {
        Encounters.detach(this)
        exporting?.reject("the screen closed before the backup was shared")
        exporting = null
        backup?.stop()
        backup = null
    }

    // ------------------------------------------------------------ Bluetooth

    /** Whether Bluetooth can be used now. Changes arrive as `bluetooth` events. */
    @PluginMethod
    fun bluetooth(call: PluginCall) {
        call.resolve(JSObject().put("state", Encounters.power(context)))
    }

    // ----------------------------------------------------------------- Back

    /**
     * The press, while the view has somewhere to go.
     *
     * Disabled at the root, so a press nobody claims is Android's own and closes
     * the app.
     */
    private val back =
        object : OnBackPressedCallback(false) {
            override fun handleOnBackPressed() {
                notifyListeners("backPressed", JSObject())
            }
        }

    /**
     * Claim the back button, or hand it back.
     *
     * A plugin call arrives on Capacitor's own thread and the dispatcher is the
     * main thread's, so the flip is posted rather than made here.
     */
    @PluginMethod
    fun setCanGoBack(call: PluginCall) {
        val can = call.getBoolean("can", false) ?: false

        activity.runOnUiThread { back.isEnabled = can }
        call.resolve()
    }

    // ---------------------------------------------------------------- Store

    @PluginMethod fun listEvents(call: PluginCall) = answer(call, "events") {
        JSArray(it.listEvents(query(call)))
    }

    @PluginMethod fun listDetails(call: PluginCall) = answer(call, "details") {
        JSArray(it.listDetails(query(call)))
    }

    @PluginMethod
    fun getEvent(call: PluginCall) {
        val id = call.getString("id") ?: return call.reject("getEvent needs an id")

        answer(call, "event") { it.getEvent(id) ?: JSONObject.NULL }
    }

    /** Drop one event from this device. Local and silent: no peer is told. */
    @PluginMethod
    fun forgetEvent(call: PluginCall) {
        val id = call.getString("id") ?: return call.reject("forgetEvent needs an id")

        answer(call, "existed") { it.forgetEvent(id) }
    }

    /** Stop recognizing somebody's device until the two next sync. */
    @PluginMethod
    fun forgetPairing(call: PluginCall) {
        val pubkey = call.getString("pubkey") ?: return call.reject("forgetPairing needs a pubkey")

        answer(call, "existed") { it.forgetPairing(pubkey) }
    }

    /** Put an event in the trash, which retracts it at once if the user wrote it. */
    @PluginMethod
    fun trash(call: PluginCall) {
        val id = call.getString("id") ?: return call.reject("trash needs an id")

        perform(call, "that could not be trashed") { it.node.trash(id) }
    }

    /** Take an event back out of the trash, which restores it for peers too if the user wrote it. */
    @PluginMethod
    fun restore(call: PluginCall) {
        val id = call.getString("id") ?: return call.reject("restore needs an id")

        perform(call, "that could not be restored") { it.node.restore(id) }
    }

    @PluginMethod fun trashed(call: PluginCall) = answer(call, "trashed") {
        JSArray(it.trashed())
    }

    /** Whether the user has let the app notify: `granted`, `denied` or `prompt`. */
    @PluginMethod
    fun notificationPermission(call: PluginCall) {
        call.resolve(JSObject().put("permission", notificationState()))
    }

    /** Ask the user to let the app notify, if they have not been asked. */
    @PluginMethod
    fun requestNotificationPermission(call: PluginCall) {
        if (notificationState() != "prompt") return notificationPermission(call)

        requestPermissionForAlias(NOTIFICATION_PERMISSION, call, "notificationsAnswered")
    }

    @PermissionCallback
    fun notificationsAnswered(call: PluginCall) = notificationPermission(call)

    // Before Android 13 there is no runtime permission, only whether the user turned them off.
    private fun notificationState(): String {
        if (android.os.Build.VERSION.SDK_INT < android.os.Build.VERSION_CODES.TIRAMISU) {
            return if (Alerts.enabled(context)) "granted" else "denied"
        }

        return when (getPermissionState(NOTIFICATION_PERMISSION)) {
            com.getcapacitor.PermissionState.GRANTED -> if (Alerts.enabled(context)) "granted" else "denied"
            com.getcapacitor.PermissionState.DENIED -> "denied"
            else -> "prompt"
        }
    }

    /** Delete everything in the trash from this device. */
    @PluginMethod
    fun emptyTrash(call: PluginCall) =
        perform(call, "the trash could not be emptied") { it.node.emptyTrash() }

    @PluginMethod fun wantedBlobs(call: PluginCall) = answer(call, "blobs") {
        JSArray(it.wantedBlobs((call.getInt("limit") ?: 32).toUInt()))
    }

    @PluginMethod
    fun getBlob(call: PluginCall) {
        val sha256 = call.getString("sha256") ?: return call.reject("getBlob needs a sha256")

        answer(call, "blob") { it.getBlob(sha256) ?: JSONObject.NULL }
    }

    @PluginMethod
    fun eventsReferencingBlob(call: PluginCall) {
        val sha256 =
            call.getString("sha256") ?: return call.reject("eventsReferencingBlob needs a sha256")

        answer(call, "ids") { JSArray(it.eventsReferencingBlob(sha256)) }
    }

    /**
     * The compiled policy every live session is bound to, as JSON, with the
     * core's own defaults where nothing is written.
     */
    @PluginMethod
    fun policy(call: PluginCall) {
        val core = this.core ?: return call.reject("policy needs a started core")

        try {
            call.resolve(JSObject().put("policy", core.node.policy()))
        } catch (error: Exception) {
            call.reject("the policy could not be read", error)
        }
    }

    @PluginMethod fun preferences(call: PluginCall) = answer(call, "preferences") { store ->
        JSArray(
            store.preferences().map {
                JSObject().put("key", it.key).put("value", it.value).put("updatedAt", it.updatedAt)
            }
        )
    }

    @PluginMethod
    fun preference(call: PluginCall) {
        val key = call.getString("key") ?: return call.reject("preference needs a key")

        answer(call, "value") { it.preference(key) ?: JSONObject.NULL }
    }

    /**
     * Write a preference and rebind live sessions under the policy it compiles
     * to, which is why this is one call rather than two the view can misorder.
     */
    @PluginMethod
    fun setPreference(call: PluginCall) {
        val key = call.getString("key") ?: return call.reject("setPreference needs a key")
        val value = call.getString("value") ?: return call.reject("setPreference needs a value")

        perform(call, "that preference could not be written") {
            it.store.setPreference(key, value)
            it.node.policyChanged()
        }
    }

    @PluginMethod
    fun clearPreference(call: PluginCall) {
        val core = this.core ?: return call.reject("clearPreference needs a started core")
        val key = call.getString("key") ?: return call.reject("clearPreference needs a key")

        try {
            val existed = core.store.clearPreference(key)

            Encounters.apply(core.node.policyChanged())
            call.resolve(JSObject().put("existed", existed))
        } catch (error: Exception) {
            call.reject("that preference could not be cleared", error)
        }
    }

    /** Run one store read and answer what it gave back under [key]. */
    private fun answer(call: PluginCall, key: String, read: (Store) -> Any) {
        val core = this.core ?: return call.reject("that call needs a started core")

        try {
            call.resolve(JSObject().put(key, read(core.store)))
        } catch (error: Exception) {
            call.reject("the store could not answer", error)
        }
    }

    /**
     * Run one core entry point the view is waiting on, and answer when the
     * actions it asked for have been carried out.
     *
     * The other half of [answer]: a store read answers with what it read, and a
     * node call answers with nothing.
     */
    private fun perform(
        call: PluginCall,
        failure: String,
        body: (Encounters.Core) -> List<Action>,
    ) {
        val core = this.core ?: return call.reject("that call needs a started core")

        try {
            Encounters.apply(body(core))
            call.resolve()
        } catch (error: Exception) {
            call.reject(failure, error)
        }
    }

    /**
     * The link the view named, which it only ever learned by being asked
     * something about it.
     *
     * A link is a `ULong` the shell assigned, so a negative one is not a link
     * this device ever handed out — refused here rather than wrapping into one
     * it never issued.
     */
    private fun link(call: PluginCall) =
        call.getInt("link")?.takeIf { it >= 0 }?.let { LinkId(it.toULong()) }

    /**
     * The query the view named, which is a filter plus provenance it may not
     * smuggle onto one.
     */
    private fun query(call: PluginCall) =
        Query(
            filter = call.getString("filter"),
            seenSince = call.getLong("seenSince"),
            seenUntil = call.getLong("seenUntil"),
            seenFrom = call.getArray("seenFrom")?.toList<String>(),
            order = if (call.getString("order") == "seenAt") Order.SEEN_AT else Order.CREATED_AT,
        )

    // ----------------------------------------------------------------- View

    override fun notify(event: String, data: JSObject) = notifyListeners(event, data)

    /**
     * Put the backup in front of the user, and tell the core when it comes back.
     *
     * The chooser is started here rather than by the view, which is what keeps
     * the path on this side of the bridge.
     */
    override fun share(file: File): Boolean {
        val call = exporting ?: return false
        val backup = this.backup

        exporting = null

        if (backup == null) {
            call.reject("there is nowhere to offer the backup")
            return false
        }

        startActivityForResult(call, backup.chooser(file), "keyBackupClosed")

        return true
    }

    // ---------------------------------------------------------- Bookkeeping

    /** Open the core with the view in front, and answer who it runs as. */
    private fun open(call: PluginCall) {
        try {
            call.resolve(JSObject().put("identity", Encounters.open(context, foreground = true)))
        } catch (error: Exception) {
            call.reject("the core could not be opened", error)
        }
    }

    /** Write an identity and answer the npub, wiping the bytes on the way out either way. */
    private fun adopt(secret: ByteArray, call: PluginCall) {
        try {
            keystore.write(secret)
            call.resolve(JSObject().put("npub", identityNpub(secret)))
        } catch (error: Exception) {
            call.reject("the identity could not be stored", error)
        } finally {
            secret.fill(0)
        }
    }
}
