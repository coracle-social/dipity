package social.coracle.dipity

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import com.getcapacitor.JSObject
import java.io.File
import uniffi.dip_ffi.Action
import uniffi.dip_ffi.Change
import uniffi.dip_ffi.LinkId
import uniffi.dip_ffi.Node
import uniffi.dip_ffi.NodeException
import uniffi.dip_ffi.PeripheralId
import uniffi.dip_ffi.Role
import uniffi.dip_ffi.Store
import uniffi.dip_ffi.StoreObserver
import uniffi.dip_ffi.Subscription
import uniffi.dip_ffi.changeName
import uniffi.dip_ffi.outcomeName

/**
 * The core for as long as the process lives, whether or not a screen shows it.
 *
 * The store, the node, the radio and the lifecycle live here, and
 * [EncounterService] keeps the process alive. Gossip happens with both phones in
 * pockets, after Android may have destroyed the activity and the plugin with it. The
 * service opens them on its own after a restart; the plugin opens them when the
 * view starts, and attaches as the [View] while it exists.
 *
 * What only a screen wants — a store change, a pairing request, a transfer
 * prompt — goes to the attached view and is dropped when there is none. A view
 * that attaches later re-reads everything on `start`. `docs/overview.md#architecture`.
 *
 * ## The loop
 *
 * Every core entry point answers a list of `Action`, and [apply] is the one place
 * they are carried out. This is never re-entered from inside a call, because the
 * core calls nothing back but custody. An action that produces more actions
 * produces them on the next entry point, not underneath this one.
 */
object Encounters : Radio.Delegate {
    /** What only a screen can do with what the core says. */
    interface View {
        /** Hand the view one notification. */
        fun notify(event: String, data: JSObject)

        /** Offer the backup file, answering whether there was anywhere to offer it. */
        fun share(file: File): Boolean
    }

    /** The store and the node, which are opened together and dropped together. */
    data class Core(val store: Store, val node: Node)

    private lateinit var context: Context

    private val keystore by lazy { Keystore(context) }
    private val radio by lazy { Radio(context, this) }

    /** The core, once there is an identity to open it under. */
    @Volatile
    var core: Core? = null
        private set

    private var lifecycle: Lifecycle? = null

    /** The store registration, live until the core closes. */
    private var watching: Subscription? = null

    /** The screen, while there is one. */
    @Volatile private var view: View? = null

    /** The plugin, while its activity lives. */
    fun attach(view: View) {
        this.view = view
    }

    /** The plugin is going, and nothing here goes with it. */
    fun detach(view: View) {
        if (this.view === view) this.view = null
    }

    /**
     * Open the core if it is not open, and answer the identity it runs as.
     *
     * `foreground` is whether a screen is in front, which only the caller knows:
     * the view starting it says yes, the service reviving it says no.
     */
    @Synchronized
    fun open(context: Context, foreground: Boolean): String {
        bind(context)

        core?.let {
            return it.node.identity()
        }

        val directory = storeDirectory()
        val store = Store.open(directory.absolutePath)
        val node = Node.open(store, KeystoreCustody(keystore), directory.absolutePath)

        core = Core(store, node)

        // Registered before the first tick so that nothing the core does on the way up goes unheard.
        watching = store.observe(StoreChanges)

        EncounterService.start(this.context)

        val lifecycle =
            Lifecycle(
                this.context,
                battery = { level -> drive { it.battery(level) } },
                tick = { drive { it.tick() } },
            )

        // Assigned before it is started: the first battery report may answer a `WakeAt`.
        this.lifecycle = lifecycle
        lifecycle.start()

        radio.foreground(foreground)
        apply(if (foreground) node.notifyForegrounded() else node.notifyBackgrounded())

        // Nothing is scanning or advertising until the core says so, and it says so on the first tick.
        apply(node.tick())

        return node.identity()
    }

    /**
     * Open the core for a process nobody opened it in, if there is an identity
     * to open it under and the radio may run: the service restarted by the
     * system after it was killed.
     */
    fun revive(context: Context) {
        bind(context)

        if (!keystore.has() || !radioPermitted()) return

        try {
            open(context, foreground = false)
        } catch (error: Exception) {
            android.util.Log.e("dip", "the core could not be reopened", error)
        }
    }

    /**
     * Drop the node and everything driving it so that [open] can run again.
     *
     * Closing twice is closing once, and the receivers are unregistered exactly
     * as often as they were registered, because every field [open] set is cleared.
     */
    @Synchronized
    fun close() {
        watching?.stop()
        lifecycle?.stop()

        if (::context.isInitialized) {
            radio.stop()
            EncounterService.stop(context)
        }

        watching = null
        lifecycle = null
        core = null
    }

    /**
     * Run one core entry point nobody is waiting on, and carry out what it
     * answered.
     *
     * A [NodeException.Link] is the core refusing to carry on with that link, so
     * it goes; anything else is logged and the loop continues.
     */
    fun drive(call: (Node) -> List<Action>) {
        val node = core?.node ?: return

        try {
            apply(call(node))
        } catch (error: NodeException.Link) {
            radio.disconnect(error.link.value)
        } catch (error: Exception) {
            android.util.Log.e("dip", "the core refused a radio event", error)
        }
    }

    /** Carry out what the core asked for, in the order it asked. */
    fun apply(actions: List<Action>) {
        for (action in actions) {
            when (action) {
                is Action.Scan -> radio.scan(action.on)
                is Action.Advertise -> radio.advertise(action.on)
                is Action.Connect -> radio.connect(action.peripheral.value)
                is Action.WaitFor -> radio.waitFor(action.peripheral.value)
                is Action.StopWaiting -> radio.stopWaiting(action.peripheral.value)
                is Action.Disconnect -> {
                    // The view hears about a link the core ended as it does about one the radio lost.
                    radio.disconnect(action.link.value)
                    notify("linkClosed", JSObject().put("link", action.link.value.toLong()))
                }
                is Action.Send -> radio.send(action.link.value, action.fragment)
                is Action.RequestApproval ->
                    notify(
                        "requestApproval",
                        JSObject()
                            .put("link", action.link.value.toLong())
                            .put("code", action.code.toLong()),
                    )
                is Action.PeerIdentified ->
                    notify(
                        "peerIdentified",
                        JSObject()
                            .put("link", action.link.value.toLong())
                            .put("pubkey", action.pubkey)
                            .put("code", action.code.toLong())
                            .put("dialed", action.dialed)
                            .put("recognized", action.recognized),
                    )
                is Action.ConfirmIdentityTransfer ->
                    notify(
                        "confirmIdentityTransfer",
                        JSObject()
                            .put("link", action.link.value.toLong())
                            .put("code", action.code.toLong()),
                    )
                is Action.IdentityTransfer ->
                    notify(
                        "identityTransfer",
                        JSObject()
                            .put("link", action.link.value.toLong())
                            .put("outcome", outcomeName(action.outcome)),
                    )
                is Action.ShareKeyBackup -> {
                    // A backup with nowhere to go is deleted rather than left on disk.
                    if (view?.share(File(action.path)) != true) drive { it.keyExportFinished() }
                }
                is Action.Notify -> Alerts.post(context, action.announcement)
                is Action.WakeAt -> lifecycle?.wake(action.at)
                is Action.SendBulk -> radio.sendBulk(action.link.value, action.fragment)
                is Action.PublishL2cap -> radio.publishL2cap(action.link.value)
                is Action.OpenL2cap -> radio.openL2cap(action.link.value, action.psm)
            }
        }
    }

    /** Hand a notification to the view, if there is one to hear it. */
    fun notify(event: String, data: JSObject) {
        view?.notify(event, data)
    }

    /** Whether the user still grants what the radio needs, which they may have revoked since. */
    private fun radioPermitted() =
        listOf(
                Manifest.permission.BLUETOOTH_SCAN,
                Manifest.permission.BLUETOOTH_ADVERTISE,
                Manifest.permission.BLUETOOTH_CONNECT,
            )
            .all { context.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED }

    /** The context everything here runs against, and the log, before anything can log. */
    private fun bind(context: Context) {
        if (::context.isInitialized) return

        this.context = context.applicationContext
        Logcat.install(this.context)
    }

    /**
     * Where the store and the blobs live, which nothing backs up or transfers:
     * `event_seen` is a record of who the user was near.
     * `docs/storage.md#the-sqlite-store`.
     */
    private fun storeDirectory(): File {
        val directory = File(context.noBackupFilesDir, "dip")

        directory.mkdirs()

        return directory
    }

    /** Tells the view which group of tables moved so that it re-reads what it shows. */
    private object StoreChanges : StoreObserver {
        override fun changed(group: Change) {
            notify("storeChanged", JSObject().put("group", changeName(group)))
        }
    }

    // ---------------------------------------------------------------- Radio

    override fun saw(peripheral: String, rssi: Short) = drive {
        it.peripheralSeen(PeripheralId(peripheral), rssi)
    }

    override fun dialFailed(peripheral: String) = drive { it.dialFailed(PeripheralId(peripheral)) }

    override fun notOurs(peripheral: String) = drive { it.notOurs(PeripheralId(peripheral)) }

    /** The app came to the front or left it, which the radio's modes and the gate both follow. */
    fun foreground(on: Boolean) {
        if (!::context.isInitialized) return

        radio.foreground(on)
        if (on) Alerts.clear(context)
        drive { if (on) it.notifyForegrounded() else it.notifyBackgrounded() }
    }

    override fun linkUp(link: ULong, peripheral: String?, dialer: Boolean, mtu: UInt) = drive {
        it.linkUp(
            LinkId(link),
            peripheral?.let(::PeripheralId),
            if (dialer) Role.DIALER else Role.RECEIVER,
            mtu,
        )
    }

    // The view is told too: a screen naming a link cannot offer over a dead one.
    override fun linkDown(link: ULong) {
        notify("linkClosed", JSObject().put("link", link.toLong()))

        drive { it.linkDown(LinkId(link)) }
    }

    override fun received(link: ULong, bytes: ByteArray) = drive {
        it.bytesReceived(LinkId(link), bytes)
    }

    override fun wrote(link: ULong) = drive { it.writeComplete(LinkId(link)) }

    override fun published(link: ULong, psm: UShort) = drive { it.l2capPublished(LinkId(link), psm) }

    override fun bulkUp(link: ULong, mtu: UInt) = drive { it.l2capOpened(LinkId(link), mtu) }

    override fun bulkDown(link: ULong) = drive { it.l2capUnavailable(LinkId(link)) }

    override fun bulkReceived(link: ULong, bytes: ByteArray) = drive {
        it.bulkReceived(LinkId(link), bytes)
    }

    override fun bulkWrote(link: ULong) = drive { it.bulkWriteComplete(LinkId(link)) }

    // Nothing to tell the core: it asks to scan regardless, and the radio keeps the request standing.
    override fun power(state: String) = notify("bluetooth", JSObject().put("state", state))

    /** Whether Bluetooth can be used now, open core or not. */
    fun power(context: Context): String {
        bind(context)

        return radio.power()
    }
}
