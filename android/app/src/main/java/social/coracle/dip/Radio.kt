package social.coracle.dip

import android.annotation.SuppressLint
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
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.Context
import android.os.ParcelUuid
import java.util.UUID
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
 * Callers hold the runtime permissions; every entry point here is annotated
 * rather than checking, because a scan that silently returns nothing because a
 * permission was never asked for is the failure mode this design cannot see.
 */
@SuppressLint("MissingPermission")
class Radio(private val context: Context, private val delegate: Delegate) {
    /** What the radio reports. Every one of these is a core entry point. */
    interface Delegate {
        fun saw(peripheral: String, rssi: Short)

        fun linkUp(link: ULong, peripheral: String?, dialer: Boolean, mtu: UInt)

        fun linkDown(link: ULong)

        fun received(link: ULong, bytes: ByteArray)

        fun wrote(link: ULong)
    }

    private val service = UUID.fromString(serviceUuid())
    private val characteristicId = UUID.fromString(characteristicUuid())

    private val manager = context.getSystemService(BluetoothManager::class.java)
    private val adapter = manager.adapter

    private var server: BluetoothGattServer? = null
    private var characteristic: BluetoothGattCharacteristic? = null

    /** Every live link, both roles, keyed the way the core names them. */
    private val links = mutableMapOf<ULong, Link>()

    /** Devices seen but not yet dialed, so a `Connect` can name one. */
    private val seen = mutableMapOf<String, BluetoothDevice>()

    private var nextLink = 1UL

    /** One connection, from either side. */
    private sealed interface Link {
        /** This device dialed, so it holds the client and its characteristic. */
        data class Dialed(val gatt: BluetoothGatt, val characteristic: BluetoothGattCharacteristic) :
            Link

        /** The peer dialed, so it is a subscriber on our GATT server. */
        data class Received(val device: BluetoothDevice) : Link
    }

    // --------------------------------------------------------------- Actions

    fun scan(on: Boolean) {
        val scanner = adapter?.bluetoothLeScanner ?: return

        if (on) {
            scanner.startScan(
                listOf(ScanFilter.Builder().setServiceUuid(ParcelUuid(service)).build()),
                ScanSettings.Builder()
                    .setScanMode(ScanSettings.SCAN_MODE_LOW_POWER)
                    // One sighting per device: the scheduler decides when to
                    // look again, not the radio.
                    .setCallbackType(ScanSettings.CALLBACK_TYPE_FIRST_MATCH)
                    .build(),
                scanning,
            )
        } else {
            scanner.stopScan(scanning)
        }
    }

    fun advertise(on: Boolean) {
        val advertiser = adapter?.bluetoothLeAdvertiser ?: return

        if (on) {
            openServer()
            advertiser.startAdvertising(
                AdvertiseSettings.Builder()
                    .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_POWER)
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

    fun connect(peripheral: String) {
        val device = seen[peripheral] ?: return

        device.connectGatt(context, false, client, BluetoothDevice.TRANSPORT_LE)
    }

    fun disconnect(link: ULong) {
        when (val held = links.remove(link)) {
            is Link.Dialed -> held.gatt.disconnect()
            is Link.Received -> server?.cancelConnection(held.device)
            null -> Unit
        }
    }

    /** Write one fragment, already sized to this link's MTU by the core. */
    fun send(link: ULong, fragment: ByteArray) {
        when (val held = links[link]) {
            // Acknowledged, because ordered reliable delivery is what the
            // control and sync channels are framed against.
            is Link.Dialed -> write(held, fragment)
            is Link.Received -> notify(held.device, fragment)
            null -> Unit
        }
    }

    /** Stop both roles and drop every link. */
    fun stop() {
        scan(false)
        advertise(false)
        links.keys.toList().forEach(::disconnect)
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
            override fun onScanResult(callbackType: Int, result: ScanResult) {
                seen[result.device.address] = result.device
                delegate.saw(result.device.address, result.rssi.toShort())
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
            override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, state: Int) {
                if (state == BluetoothProfile.STATE_CONNECTED) {
                    // The MTU is negotiated before the service is used, so the
                    // fragment size the core is told is the one it gets.
                    gatt.requestMtu(MTU)
                } else {
                    linkFor(gatt)?.let {
                        links.remove(it)
                        delegate.linkDown(it)
                    }
                    gatt.close()
                }
            }

            override fun onMtuChanged(gatt: BluetoothGatt, mtu: Int, status: Int) {
                gatt.discoverServices()
            }

            override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
                val found = gatt.getService(service)?.getCharacteristic(characteristicId)

                if (found == null) return gatt.disconnect()

                gatt.setCharacteristicNotification(found, true)
                found.getDescriptor(CLIENT_CONFIGURATION)?.let {
                    gatt.writeDescriptor(it, BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE)
                }

                val link = take()
                links[link] = Link.Dialed(gatt, found)

                delegate.linkUp(link, gatt.device.address, true, (MTU - ATT_OVERHEAD).toUInt())
            }

            override fun onCharacteristicChanged(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                value: ByteArray,
            ) {
                linkFor(gatt)?.let { delegate.received(it, value) }
            }

            override fun onCharacteristicWrite(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                status: Int,
            ) {
                // A failed write is still an answer: the core will not stall
                // waiting for one that is never coming.
                linkFor(gatt)?.let(delegate::wrote)
            }
        }

    // --------------------------------------------------------- GATT server

    private val gattServer =
        object : BluetoothGattServerCallback() {
            override fun onConnectionStateChange(device: BluetoothDevice, status: Int, state: Int) {
                if (state != BluetoothProfile.STATE_CONNECTED) {
                    linkFor(device)?.let {
                        links.remove(it)
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
            ) {
                // Subscribing is what makes a link: before it there is a
                // connection with no way to write back over it.
                if (linkFor(device) == null) {
                    val link = take()
                    links[link] = Link.Received(device)

                    delegate.linkUp(link, null, false, (MTU - ATT_OVERHEAD).toUInt())
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
            ) {
                linkFor(device)?.let { delegate.received(it, value) }

                if (responseNeeded) {
                    server?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, 0, null)
                }
            }

            override fun onNotificationSent(device: BluetoothDevice, status: Int) {
                linkFor(device)?.let(delegate::wrote)
            }
        }

    private companion object {
        /** The client characteristic configuration descriptor, as BLE fixes it. */
        val CLIENT_CONFIGURATION: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")

        /** What the MTU is negotiated up to, which is the BLE maximum. */
        const val MTU = 517

        /** The ATT header a write carries, off the negotiated MTU. */
        const val ATT_OVERHEAD = 3
    }
}
