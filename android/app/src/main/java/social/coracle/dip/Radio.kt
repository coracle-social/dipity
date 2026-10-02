package social.coracle.dip

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothGattServer
import android.bluetooth.BluetoothGattServerCallback
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.BluetoothServerSocket
import android.bluetooth.BluetoothSocket
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.os.ParcelUuid
import java.io.Closeable
import java.io.IOException
import java.util.UUID
import java.util.concurrent.Callable
import java.util.concurrent.Executors
import androidx.core.content.ContextCompat
import kotlin.concurrent.thread
import uniffi.dip_ffi.characteristicUuid
import uniffi.dip_ffi.serviceUuid

/**
 * The radio, and nothing else.
 *
 * The core decides everything about who to dial, when, and what to send; this
 * moves bytes and reports what the hardware did. `docs/transport.md#link-layer`
 * and `docs/discovery.md` are the specification, and the two rules that follow
 * from them are:
 *
 * - **Both roles at once.** An advertiser and a scanner, and a GATT server and
 *   a GATT client, against the same one service and one characteristic — so a
 *   meeting connects whichever way round the two phones happen to be.
 * - **Nothing here is a policy.** The RSSI floor, the six-link cap, the rate
 *   limiting and the backoff tiers are `node::scheduler`'s. Reimplementing any
 *   of them in Kotlin is how the two platforms start disagreeing about who is
 *   worth dialing.
 *
 * One thread owns the state: `links`, `seen`, `bulk`, `listening`, `nextLink`
 * and the GATT server are read and written on [queue], and nothing reaches them
 * except through [confined] — every entry point below, and every callback the
 * hardware makes. That is the rule the iOS shell gets for free from
 * CoreBluetooth's nil queue, stated here because Kotlin gives nothing for free.
 * A call that blocks — an `accept`, a dial — runs on a thread of its own and
 * posts back only what it found.
 *
 * Callers hold the runtime permissions; every entry point here is annotated
 * rather than checking, because a scan that silently returns nothing because a
 * permission was never asked for is the failure mode this design cannot see.
 */
@SuppressLint("MissingPermission")
class Radio(private val context: Context, private val delegate: Delegate) {
    /** What the radio reports. Every one of these is a core entry point. */
    interface Delegate {
        fun saw(peripheral: String, rssi: Short)

        fun dialFailed(peripheral: String)

        fun linkUp(link: ULong, peripheral: String?, dialer: Boolean, mtu: UInt)

        fun linkDown(link: ULong)

        fun received(link: ULong, bytes: ByteArray)

        fun wrote(link: ULong)

        fun published(link: ULong, psm: UShort)

        fun bulkUp(link: ULong, mtu: UInt)

        fun bulkDown(link: ULong)

        fun bulkReceived(link: ULong, bytes: ByteArray)

        fun bulkWrote(link: ULong)

        fun power(state: String)
    }

    private val service = UUID.fromString(serviceUuid())
    private val characteristicId = UUID.fromString(characteristicUuid())

    private val manager = context.getSystemService(BluetoothManager::class.java)
    private val adapter = manager.adapter

    private var server: BluetoothGattServer? = null
    private var characteristic: BluetoothGattCharacteristic? = null

    /**
     * The thread this radio's own state lives on.
     *
     * Capacitor's binder threads, the GATT callback threads, each publication's
     * accept thread and each channel's reader and writer all reach the fields
     * below, so every one of those entries is posted here instead of being
     * synchronized.
     */
    private val queue = Executors.newSingleThreadExecutor { work -> Thread(work, "dip-radio") }

    /** Which thread that is, so a nested call runs now rather than later. */
    private val owner = queue.submit(Callable { Thread.currentThread() }).get()

    /** Every live link, both roles, keyed the way the core names them. */
    private val links = mutableMapOf<ULong, Link>()

    /** Devices seen but not yet dialed, so a `Connect` can name one. */
    private val seen = mutableMapOf<String, BluetoothDevice>()

    /** The open L2CAP channel on each link, for as long as one is. */
    private val bulk = mutableMapOf<ULong, BulkChannel>()

    /** The server socket each publishing link is waiting for its peer on. */
    private val listening = mutableMapOf<ULong, BluetoothServerSocket>()

    /** The ATT MTU each connection negotiated, by device address, until its link is made. */
    private val negotiated = mutableMapOf<String, Int>()

    private var nextLink = 1UL

    /** What the core last asked of the radio, applied again whenever Bluetooth comes back on. */
    private var wantsScan = false
    private var wantsAdvertise = false

    /** Whether the app is in front, which buys the fast scan and advertising modes. */
    private var foreground = false

    /** When each device was last reported, so a peer advertising ten times a second is reported once. */
    private val reported = mutableMapOf<String, Long>()

    /**
     * Bluetooth being switched off or on, which ends every link and stops every
     * scan and advertisement, and on the way back means starting them again.
     */
    private val switched =
        object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) = confined {
                when (intent.getIntExtra(BluetoothAdapter.EXTRA_STATE, -1)) {
                    BluetoothAdapter.STATE_ON -> {
                        scan(wantsScan)
                        advertise(wantsAdvertise)
                    }
                    BluetoothAdapter.STATE_OFF -> {
                        server?.close()
                        server = null
                    }
                }

                delegate.power(power())
            }
        }

    init {
        ContextCompat.registerReceiver(
            context,
            switched,
            IntentFilter(BluetoothAdapter.ACTION_STATE_CHANGED),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
    }

    /**
     * Whether Bluetooth can be used, as the view words it: `on`, `off`, `denied`
     * when the permission is missing, or `unsupported`.
     */
    fun power(): String =
        when {
            adapter == null -> "unsupported"
            context.checkSelfPermission(Manifest.permission.BLUETOOTH_CONNECT) !=
                PackageManager.PERMISSION_GRANTED -> "denied"
            adapter.isEnabled -> "on"
            else -> "off"
        }

    /** One connection, from either side. */
    private sealed interface Link {
        /** This device dialed, so it holds the client and its characteristic. */
        data class Dialed(val gatt: BluetoothGatt, val characteristic: BluetoothGattCharacteristic) :
            Link

        /** The peer dialed, so it is a subscriber on our GATT server. */
        data class Received(val device: BluetoothDevice) : Link
    }

    /**
     * Run [work] on [queue], or run it now if this already is [queue]'s thread.
     *
     * Running a nested call inline is what keeps one entry point calling another
     * — `stop` calling `disconnect`, a delegate answering with an action — in the
     * order it reads in.
     */
    private fun confined(work: () -> Unit) {
        if (Thread.currentThread() === owner) work() else queue.execute(work)
    }

    // --------------------------------------------------------------- Actions

    fun scan(on: Boolean) = confined {
        wantsScan = on

        val scanner = adapter?.bluetoothLeScanner ?: return@confined

        // Restarting is how the mode changes, and starting twice is an error the callback would swallow.
        scanner.stopScan(scanning)

        if (on) {
            scanner.startScan(
                listOf(ScanFilter.Builder().setServiceUuid(ParcelUuid(service)).build()),
                ScanSettings.Builder()
                    .setScanMode(
                        if (foreground) ScanSettings.SCAN_MODE_LOW_LATENCY
                        else ScanSettings.SCAN_MODE_BALANCED
                    )
                    // Every advertisement, not the first: a peer the scheduler queued keeps being seen while it is there.
                    .setCallbackType(ScanSettings.CALLBACK_TYPE_ALL_MATCHES)
                    .build(),
                scanning,
            )
        }
    }

    /**
     * The app came to the front or left it. In front, scanning and advertising
     * run in their fastest modes, so a peer is found in a second or two; behind,
     * in balanced ones, so the pocket does not pay for it.
     */
    fun foreground(on: Boolean) = confined {
        if (foreground == on) return@confined

        foreground = on

        if (wantsScan) scan(true)
        if (wantsAdvertise) advertise(true)
    }

    fun advertise(on: Boolean) = confined {
        wantsAdvertise = on

        val advertiser = adapter?.bluetoothLeAdvertiser ?: return@confined

        advertiser.stopAdvertising(advertising)

        if (on) {
            openServer()
            advertiser.startAdvertising(
                AdvertiseSettings.Builder()
                    .setAdvertiseMode(
                        if (foreground) AdvertiseSettings.ADVERTISE_MODE_LOW_LATENCY
                        else AdvertiseSettings.ADVERTISE_MODE_BALANCED
                    )
                    .setConnectable(true)
                    .build(),
                // The service UUID and nothing else — `docs/discovery.md`.
                AdvertiseData.Builder()
                    .addServiceUuid(ParcelUuid(service))
                    .setIncludeDeviceName(false)
                    .build(),
                advertising,
            )
        } else {
            advertiser.stopAdvertising(advertising)
        }
    }

    fun connect(peripheral: String) = confined {
        val device = seen[peripheral] ?: return@confined

        device.connectGatt(context, false, client, BluetoothDevice.TRANSPORT_LE)
    }

    fun disconnect(link: ULong) = confined {
        val held = links[link]

        forget(link)

        when (held) {
            is Link.Dialed -> held.gatt.disconnect()
            is Link.Received -> server?.cancelConnection(held.device)
            null -> Unit
        }
    }

    /**
     * Publish a channel for the peer on [link] to connect to.
     *
     * Only a link the peer dialed can: listening is the GATT server's side of
     * the connection, which is the role this device holds on one it received.
     */
    fun publishL2cap(link: ULong) = confined {
        val device =
            (links[link] as? Link.Received)?.device ?: return@confined delegate.bulkDown(link)
        val server =
            try {
                adapter?.listenUsingInsecureL2capChannel()
                    ?: return@confined delegate.bulkDown(link)
            } catch (error: IOException) {
                return@confined delegate.bulkDown(link)
            }

        listening[link] = server
        delegate.published(link, server.psm.toUShort())

        thread(name = "dip-l2cap-accept-$link") { accept(link, server, device) }
    }

    /** Open the channel the peer published at [psm]. */
    fun openL2cap(link: ULong, psm: UShort) = confined {
        val device =
            (links[link] as? Link.Dialed)?.gatt?.device ?: return@confined delegate.bulkDown(link)

        thread(name = "dip-l2cap-open-$link") {
            val socket =
                try {
                    device.createInsecureL2capChannel(psm.toInt()).also { it.connect() }
                } catch (error: IOException) {
                    null
                }

            confined { if (socket == null) delegate.bulkDown(link) else adopt(link, socket) }
        }
    }

    /** Write one bulk fragment, already sized and length-prefixed by the core. */
    fun sendBulk(link: ULong, fragment: ByteArray) = confined {
        val channel = bulk[link] ?: return@confined delegate.bulkDown(link)

        channel.write(fragment)
    }

    /** Write one fragment, already sized to this link's MTU by the core. */
    fun send(link: ULong, fragment: ByteArray) = confined {
        when (val held = links[link]) {
            // Acknowledged, because ordered reliable delivery is what the
            // control and sync channels are framed against.
            is Link.Dialed -> write(held, fragment)
            is Link.Received -> notify(held.device, fragment)
            null -> Unit
        }
    }

    /** Stop both roles and drop every link. */
    fun stop() = confined {
        scan(false)
        advertise(false)
        links.keys.toList().forEach(::disconnect)
        bulk.keys.toList().forEach(::forget)
        server?.close()
        server = null
    }

    // ------------------------------------------------------------ Bookkeeping

    private fun take(): ULong = nextLink++

    private fun linkFor(gatt: BluetoothGatt): ULong? =
        links.entries.firstOrNull { (_, held) -> held is Link.Dialed && held.gatt == gatt }?.key

    private fun linkFor(device: BluetoothDevice): ULong? =
        links.entries
            .firstOrNull { (_, held) -> held is Link.Received && held.device.address == device.address }
            ?.key

    /**
     * Drop a link and everything hanging off it.
     *
     * The link goes first, so a channel closing on its way out is not reported
     * as an upgrade this device lost.
     */
    private fun forget(link: ULong) {
        when (val held = links.remove(link)) {
            is Link.Dialed -> negotiated.remove(held.gatt.device.address)
            is Link.Received -> negotiated.remove(held.device.address)
            null -> Unit
        }
        bulk.remove(link)?.close()
        listening.remove(link)?.let(::shut)
    }

    /** Wait for the peer this PSM was published for, and nobody else. */
    private fun accept(link: ULong, server: BluetoothServerSocket, device: BluetoothDevice) {
        val socket =
            try {
                server.accept(ACCEPT_TIMEOUT)
            } catch (error: IOException) {
                null
            }

        shut(server)

        confined {
            listening.remove(link)

            if (socket == null || socket.remoteDevice.address != device.address) {
                socket?.let(::shut)

                return@confined delegate.bulkDown(link)
            }

            adopt(link, socket)
        }
    }

    /** Take an open channel over, and tell the core bulk can move onto it. */
    private fun adopt(link: ULong, socket: BluetoothSocket) {
        val channel = BulkChannel(link, socket, pump)

        bulk[link] = channel
        channel.open()

        delegate.bulkUp(link, socket.maxTransmitPacketSize.toUInt())
    }

    /** One fragment's size on [device]'s connection: its ATT MTU less the header, within an attribute. */
    private fun fragmentSize(device: BluetoothDevice): UInt =
        ((negotiated[device.address] ?: DEFAULT_MTU) - ATT_OVERHEAD).coerceAtMost(MAX_ATTRIBUTE).toUInt()

    private fun shut(closeable: Closeable) {
        try {
            closeable.close()
        } catch (error: IOException) {
            // Closing a socket that is already gone is the outcome asked for.
        }
    }

    private val pump =
        object : BulkChannel.Delegate {
            override fun bulkRead(channel: BulkChannel, bytes: ByteArray) = confined {
                delegate.bulkReceived(channel.link, bytes)
            }

            override fun bulkWrote(channel: BulkChannel) = confined {
                delegate.bulkWrote(channel.link)
            }

            override fun bulkClosed(channel: BulkChannel) = confined {
                if (bulk[channel.link] === channel) {
                    bulk.remove(channel.link)
                    delegate.bulkDown(channel.link)
                }
            }
        }

    private fun openServer() {
        if (server != null) return

        val served = BluetoothGattService(service, BluetoothGattService.SERVICE_TYPE_PRIMARY)
        val exposed =
            BluetoothGattCharacteristic(
                characteristicId,
                BluetoothGattCharacteristic.PROPERTY_NOTIFY or
                    BluetoothGattCharacteristic.PROPERTY_WRITE or
                    BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE,
                BluetoothGattCharacteristic.PERMISSION_WRITE,
            )

        exposed.addDescriptor(
            BluetoothGattDescriptor(
                CLIENT_CONFIGURATION,
                BluetoothGattDescriptor.PERMISSION_READ or
                    BluetoothGattDescriptor.PERMISSION_WRITE,
            )
        )

        served.addCharacteristic(exposed)
        characteristic = exposed
        server = manager.openGattServer(context, gattServer).also { it.addService(served) }
    }

    private fun write(held: Link.Dialed, fragment: ByteArray) {
        held.gatt.writeCharacteristic(
            held.characteristic,
            fragment,
            BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT,
        )
    }

    private fun notify(device: BluetoothDevice, fragment: ByteArray) {
        val exposed = characteristic ?: return
        val gatt = server ?: return

        gatt.notifyCharacteristicChanged(device, exposed, false, fragment)
    }

    // ------------------------------------------------------------- Scanning

    private val scanning =
        object : ScanCallback() {
            override fun onScanResult(callbackType: Int, result: ScanResult) = confined {
                val address = result.device.address
                val now = System.currentTimeMillis()

                seen[address] = result.device

                if (now - (reported[address] ?: 0L) < REPORT_INTERVAL) return@confined

                reported[address] = now
                delegate.saw(address, result.rssi.toShort())
            }

            override fun onScanFailed(errorCode: Int) {
                android.util.Log.e("dip", "scanning failed to start: $errorCode")
            }
        }

    private val advertising =
        object : AdvertiseCallback() {
            override fun onStartFailure(errorCode: Int) {
                // Nothing to report to the core: it asked to be discoverable and
                // is not, which reads to a peer as a device that is not here.
            }
        }

    // --------------------------------------------------------- GATT client

    private val client =
        object : BluetoothGattCallback() {
            override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, state: Int) =
                confined {
                    if (state == BluetoothProfile.STATE_CONNECTED) {
                        // The MTU is negotiated before the service is used, so
                        // the fragment size the core is told is the one it gets.
                        gatt.requestMtu(MTU)
                    } else {
                        val link = linkFor(gatt)

                        // No link means the dial never came up, which the scheduler retries shortly.
                        if (link == null) {
                            delegate.dialFailed(gatt.device.address)
                        } else {
                            forget(link)
                            delegate.linkDown(link)
                        }

                        gatt.close()
                    }
                }

            override fun onMtuChanged(gatt: BluetoothGatt, mtu: Int, status: Int) = confined {
                if (status == BluetoothGatt.GATT_SUCCESS) negotiated[gatt.device.address] = mtu

                gatt.discoverServices()
            }

            override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) = confined {
                val found = gatt.getService(service)?.getCharacteristic(characteristicId)
                val subscription = found?.getDescriptor(CLIENT_CONFIGURATION)

                if (found == null || subscription == null) return@confined gatt.disconnect()

                gatt.setCharacteristicNotification(found, true)
                gatt.writeDescriptor(subscription, BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE)
            }

            // Android runs one GATT operation at a time, so the link is up once the subscription lands.
            override fun onDescriptorWrite(
                gatt: BluetoothGatt,
                descriptor: BluetoothGattDescriptor,
                status: Int,
            ) = confined {
                if (status != BluetoothGatt.GATT_SUCCESS) return@confined gatt.disconnect()

                val link = take()
                links[link] = Link.Dialed(gatt, descriptor.characteristic)

                delegate.linkUp(link, gatt.device.address, true, fragmentSize(gatt.device))
            }

            override fun onCharacteristicChanged(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                value: ByteArray,
            ) = confined { linkFor(gatt)?.let { delegate.received(it, value) } }

            override fun onCharacteristicWrite(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                status: Int,
            ) = confined {
                // A failed write is still an answer: the core will not stall
                // waiting for one that is never coming.
                linkFor(gatt)?.let(delegate::wrote)
            }
        }

    // --------------------------------------------------------- GATT server

    private val gattServer =
        object : BluetoothGattServerCallback() {
            override fun onConnectionStateChange(device: BluetoothDevice, status: Int, state: Int) =
                confined {
                    if (state != BluetoothProfile.STATE_CONNECTED) {
                        linkFor(device)?.let {
                            forget(it)
                            delegate.linkDown(it)
                        }
                    }
                }

            override fun onDescriptorWriteRequest(
                device: BluetoothDevice,
                requestId: Int,
                descriptor: BluetoothGattDescriptor,
                preparedWrite: Boolean,
                responseNeeded: Boolean,
                offset: Int,
                value: ByteArray,
            ) = confined {
                // Subscribing is what makes a link: before it there is a
                // connection with no way to write back over it.
                if (linkFor(device) == null) {
                    val link = take()
                    links[link] = Link.Received(device)

                    delegate.linkUp(link, null, false, fragmentSize(device))
                }

                if (responseNeeded) {
                    server?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, 0, null)
                }
            }

            override fun onCharacteristicWriteRequest(
                device: BluetoothDevice,
                requestId: Int,
                characteristic: BluetoothGattCharacteristic,
                preparedWrite: Boolean,
                responseNeeded: Boolean,
                offset: Int,
                value: ByteArray,
            ) = confined {
                // A long write splits one fragment across requests, which the core would read as several.
                val status =
                    if (preparedWrite || offset != 0) {
                        BluetoothGatt.GATT_REQUEST_NOT_SUPPORTED
                    } else {
                        linkFor(device)?.let { delegate.received(it, value) }
                        BluetoothGatt.GATT_SUCCESS
                    }

                if (responseNeeded) {
                    server?.sendResponse(device, requestId, status, 0, null)
                }
            }

            override fun onMtuChanged(device: BluetoothDevice, mtu: Int) = confined {
                negotiated[device.address] = mtu
            }

            override fun onNotificationSent(device: BluetoothDevice, status: Int) = confined {
                linkFor(device)?.let(delegate::wrote)
            }
        }

    private companion object {
        /** The client characteristic configuration descriptor, as BLE fixes it. */
        val CLIENT_CONFIGURATION: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")

        /** What the MTU is negotiated up to, which is the BLE maximum. */
        const val MTU = 517

        /** The least time between two reports of one device, in milliseconds. */
        const val REPORT_INTERVAL = 1_000L

        /** The ATT header a write carries, off the negotiated MTU. */
        const val ATT_OVERHEAD = 3

        /** The ATT MTU a connection has before either side negotiates one. */
        const val DEFAULT_MTU = 23

        /** The longest attribute value BLE allows, whatever the MTU. */
        const val MAX_ATTRIBUTE = 512

        /**
         * How long a published channel waits for its peer, in milliseconds.
         *
         * The peer has been told the PSM and has an open GATT link to reach it
         * over, so a wait this long means it is not coming.
         */
        const val ACCEPT_TIMEOUT = 30_000
    }
}
