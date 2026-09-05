package social.coracle.dip

import android.Manifest
import com.getcapacitor.JSObject
import com.getcapacitor.Plugin
import com.getcapacitor.PluginCall
import com.getcapacitor.PluginMethod
import com.getcapacitor.annotation.CapacitorPlugin
import com.getcapacitor.annotation.Permission
import com.getcapacitor.annotation.PermissionCallback
import uniffi.dip_ffi.Action
import uniffi.dip_ffi.LinkId
import uniffi.dip_ffi.Node
import uniffi.dip_ffi.NodeException
import uniffi.dip_ffi.PeripheralId
import uniffi.dip_ffi.Role
import uniffi.dip_ffi.Store
import uniffi.dip_ffi.TransferOutcome
import uniffi.dip_ffi.coreVersion as loadedCoreVersion
import uniffi.dip_ffi.generateIdentity
import uniffi.dip_ffi.identityFromNsec
import uniffi.dip_ffi.identityNpub

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
    private val keystore by lazy { Keystore(context) }
    private val radio by lazy { Radio(context, this) }

    private var store: Store? = null
    private var node: Node? = null

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
        if (node != null) return call.resolve()

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

    /** Store and offer an event the view built, with the media it attaches. */
    @PluginMethod
    fun publish(call: PluginCall) {
        val node = this.node ?: return call.reject("publish needs a started core")
        val event = call.getString("event") ?: return call.reject("publish needs an event")
        val media =
            (0 until (call.getArray("media")?.length() ?: 0)).map {
                android.util.Base64.decode(call.getArray("media").getString(it), android.util.Base64.DEFAULT)
            }

        try {
            apply(node.publish(event, media))
            call.resolve()
        } catch (error: Exception) {
            call.reject("that event could not be published", error)
        }
    }

    /** The user answered a `requestApproval` the plugin sent up. */
    @PluginMethod
    fun approve(call: PluginCall) {
        val node = this.node ?: return call.reject("approve needs a started core")
        val link = call.getInt("link") ?: return call.reject("approve needs a link")

        try {
            apply(node.approve(LinkId(link.toULong()), call.getBoolean("approved", false) == true))
            call.resolve()
        } catch (error: Exception) {
            call.reject("that approval could not be recorded", error)
        }
    }

    override fun handleOnDestroy() {
        radio.stop()
        EncounterService.stop(context)
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
                is Action.ShareKeyBackup ->
                    notifyListeners("shareKeyBackup", JSObject().put("path", action.path))
                // Advisory. #52 is what makes this an alarm rather than a note.
                is Action.WakeAt -> notifyListeners("wakeAt", JSObject().put("at", action.at))
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
     * Run one core entry point and carry out what it answered.
     *
     * A [NodeException.Link] is the core refusing to carry on with that link, so
     * it goes; anything else is logged and the loop continues.
     */
    private fun drive(call: (Node) -> List<Action>) {
        val node = this.node ?: return

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
            val opened = Store.open(java.io.File(directory, "dip.sqlite").absolutePath)
            val node = Node.open(opened, KeystoreCustody(keystore), directory.absolutePath)

            store = opened
            this.node = node

            EncounterService.start(context)

            // Nothing is scanning or advertising until the core says so, and it
            // says so on the first tick.
            apply(node.tick())
            call.resolve(JSObject().put("identity", node.identity()))
        } catch (error: Exception) {
            call.reject("the core could not be opened", error)
        }
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
