package social.coracle.dip

import android.Manifest
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
import uniffi.dip_ffi.Node
import uniffi.dip_ffi.NodeException
import uniffi.dip_ffi.Order
import uniffi.dip_ffi.PeripheralId
import uniffi.dip_ffi.Query
import uniffi.dip_ffi.Role
import uniffi.dip_ffi.Store
import uniffi.dip_ffi.StoreObserver
import uniffi.dip_ffi.Subscription
import uniffi.dip_ffi.TransferOutcome
import uniffi.dip_ffi.changeName
import uniffi.dip_ffi.coreVersion as loadedCoreVersion
import uniffi.dip_ffi.generateIdentity
import uniffi.dip_ffi.identityFromNsec
import uniffi.dip_ffi.identityNpub
import uniffi.dip_ffi.mediaTags as coreMediaTags

/** The three permissions the radio needs on 31+, asked for together. */
private const val RADIO_PERMISSIONS = "radio"

/**
 * The webview's end of the core.
 *
 * One plugin, holding the store, the node and the radio, with a method per call
 * the view makes. Everything below the bridge is `dip_ffi`; nothing here decides
 * anything about gossip, the radio schedule or policy.
 *
 * ## The loop
 *
 * Every core entry point answers a list of `Action`, and [apply] is the one
 * place they are carried out. The core calls nothing back, so this is never
 * re-entered from inside a call — an action that produces more actions produces
 * them on the next entry point, not underneath this one.
 *
 * ## The three call shapes
 *
 * A store read answers what it read, through [answer]. A node call answers only
 * the actions it asked for, through [perform]. A radio event has nobody waiting
 * on it, through [drive]. Each takes the opened core, runs one entry point and
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
            )
        ],
)
class DipPlugin : Plugin(), Radio.Delegate {
    /**
     * The store and the node, which are opened together and dropped together.
     *
     * One field rather than two: every call the view makes needs one or both,
     * and a plugin holding half a core is a state no call site should have to
     * consider.
     */
    private data class Core(val store: Store, val node: Node)

    private val keystore by lazy { Keystore(context) }
    private val radio by lazy { Radio(context, this) }

    /** The core, once there is an identity to open it under. */
    private var core: Core? = null

    private var lifecycle: Lifecycle? = null
    private var backup: Backup? = null

    /** The store registration, live until the plugin drops it. */
    private var watching: Subscription? = null

    /** The `exportKey` call waiting on the chooser it opened. */
    private var exporting: PluginCall? = null

    /** Where the core's log goes, installed before anything can log. */
    override fun load() = Logcat.install(context)

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

    @PluginMethod
    fun deleteIdentity(call: PluginCall) {
        call.resolve(JSObject().put("existed", keystore.delete()))
    }

    // ----------------------------------------------------------------- Node

    /**
     * Open the store and the node, and start the radio.
     *
     * The view calls this once it knows there is an identity, which is what
     * makes first run a screen rather than a failed open.
     */
    @PluginMethod
    fun start(call: PluginCall) {
        if (core != null) return call.resolve()

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
            call.getArray("media")?.toList<String>().orEmpty().map {
                android.util.Base64.decode(it, android.util.Base64.DEFAULT)
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

        perform(call, "that identity could not be offered") { it.node.offerIdentity(link) }
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
        } catch (error: Exception) {
            return call.reject("the transferred identity could not be stored", error)
        }

        close()
        open(call)
    }

    /**
     * Write a key backup into the cache directory and offer it to the chooser.
     *
     * The path never comes back over the bridge: the view learns only that the
     * file was shared or dismissed. `docs/keys.md#backup`.
     */
    @PluginMethod
    fun exportKey(call: PluginCall) {
        val core = this.core ?: return call.reject("exportKey needs a started core")

        // One chooser means one call waiting on it, so whoever this displaces
        // is answered rather than left on a promise that never settles.
        exporting?.reject("another key export replaced this one")
        exporting = call

        try {
            apply(core.node.exportKey(context.cacheDir.absolutePath, call.getString("password")))
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

        drive { it.keyExportFinished() }
        call.resolve()
    }

    override fun handleOnResume() = drive { it.notifyForegrounded() }

    override fun handleOnPause() = drive { it.notifyBackgrounded() }

    override fun handleOnDestroy() = close()

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

            apply(core.node.policyChanged())
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
    private fun perform(call: PluginCall, failure: String, body: (Core) -> List<Action>) {
        val core = this.core ?: return call.reject("that call needs a started core")

        try {
            apply(body(core))
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

    /** Tells the view which group of tables moved, so it re-reads what it shows. */
    private inner class StoreChanges : StoreObserver {
        override fun changed(group: Change) {
            notifyListeners("storeChanged", JSObject().put("group", changeName(group)))
        }
    }

    // -------------------------------------------------------------- Actions

    /** Carry out what the core asked for, in the order it asked. */
    private fun apply(actions: List<Action>) {
        for (action in actions) {
            when (action) {
                is Action.Scan -> radio.scan(action.on)
                is Action.Advertise -> radio.advertise(action.on)
                is Action.Connect -> radio.connect(action.peripheral.value)
                is Action.Disconnect -> radio.disconnect(action.link.value)
                is Action.Send -> radio.send(action.link.value, action.fragment)
                is Action.RequestApproval ->
                    notifyListeners("requestApproval", JSObject().put("link", action.link.value.toLong()))
                is Action.ConfirmIdentityTransfer ->
                    notifyListeners(
                        "confirmIdentityTransfer",
                        JSObject()
                            .put("link", action.link.value.toLong())
                            .put("code", action.code.toLong()),
                    )
                is Action.IdentityTransfer ->
                    notifyListeners(
                        "identityTransfer",
                        JSObject()
                            .put("link", action.link.value.toLong())
                            .put("received", action.outcome == TransferOutcome.RECEIVED),
                    )
                is Action.ShareKeyBackup -> share(File(action.path))
                is Action.WakeAt -> lifecycle?.wake(action.at)
                // The bandwidth upgrade is bandwidth, and a link that never gets
                // one still syncs. L2CAP lands with its own change.
                is Action.SendBulk,
                is Action.PublishL2cap,
                is Action.OpenL2cap -> Unit
            }
        }
    }

    // ---------------------------------------------------------------- Radio

    override fun saw(peripheral: String, rssi: Short) = drive {
        it.peripheralSeen(PeripheralId(peripheral), rssi)
    }

    override fun linkUp(link: ULong, peripheral: String?, dialer: Boolean, mtu: UInt) = drive {
        it.linkUp(
            LinkId(link),
            peripheral?.let(::PeripheralId),
            if (dialer) Role.DIALER else Role.RECEIVER,
            mtu,
        )
    }

    override fun linkDown(link: ULong) = drive { it.linkDown(LinkId(link)) }

    override fun received(link: ULong, bytes: ByteArray) = drive {
        it.bytesReceived(LinkId(link), bytes)
    }

    override fun wrote(link: ULong) = drive { it.writeComplete(LinkId(link)) }

    /**
     * Run one core entry point nobody is waiting on, and carry out what it
     * answered.
     *
     * A [NodeException.Link] is the core refusing to carry on with that link, so
     * it goes; anything else is logged and the loop continues.
     */
    private fun drive(call: (Node) -> List<Action>) {
        val node = core?.node ?: return

        try {
            apply(call(node))
        } catch (error: NodeException.Link) {
            radio.disconnect(error.link.value)
        } catch (error: Exception) {
            android.util.Log.e("dip", "the core refused a radio event", error)
        }
    }

    // ---------------------------------------------------------- Bookkeeping

    private fun open(call: PluginCall) {
        try {
            val directory = context.filesDir
            val store = Store.open(File(directory, "dip.sqlite").absolutePath)
            val node = Node.open(store, KeystoreCustody(keystore), directory.absolutePath)

            core = Core(store, node)

            // Registered before the first tick, so nothing the core does on the
            // way up is a change the view never hears about.
            watching = store.observe(StoreChanges())

            EncounterService.start(context)

            val lifecycle =
                Lifecycle(
                    context,
                    battery = { level -> drive { it.battery(level) } },
                    tick = { drive { it.tick() } },
                )

            // Assigned before it is started: the first battery report is an
            // entry point like any other, and what it answers may be a `WakeAt`.
            this.lifecycle = lifecycle
            lifecycle.start()

            backup = Backup(context)

            // The view is what calls `start`, and the view runs in front, so
            // this is where presence is first reported rather than guessed.
            apply(node.notifyForegrounded())

            // Nothing is scanning or advertising until the core says so, and it
            // says so on the first tick.
            apply(node.tick())
            call.resolve(JSObject().put("identity", node.identity()))
        } catch (error: Exception) {
            call.reject("the core could not be opened", error)
        }
    }

    /**
     * Put the backup in front of the user, and tell the core when it comes back.
     *
     * The chooser is started here rather than by the view, which is what keeps
     * the path on this side of the bridge.
     */
    private fun share(file: File) {
        val call = exporting ?: return
        val backup = this.backup

        exporting = null

        if (backup == null) return call.reject("there is nowhere to offer the backup")

        startActivityForResult(call, backup.chooser(file), "keyBackupClosed")
    }

    /**
     * Drop the node and everything driving it, so [open] can run again.
     *
     * Every field [open] set is cleared, so closing twice is closing once and
     * the receivers are unregistered exactly as often as they were registered.
     */
    private fun close() {
        exporting?.reject("the core closed before the backup was shared")
        watching?.stop()
        lifecycle?.stop()
        backup?.stop()
        radio.stop()
        EncounterService.stop(context)

        exporting = null
        watching = null
        lifecycle = null
        backup = null
        core = null
    }

    /** Write an identity and answer the npub, which is all the view is owed. */
    private fun adopt(secret: ByteArray, call: PluginCall) {
        try {
            keystore.write(secret)
            call.resolve(JSObject().put("npub", identityNpub(secret)))
        } catch (error: Exception) {
            call.reject("the identity could not be stored", error)
        }
    }
}
