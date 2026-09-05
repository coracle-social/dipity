import CoreBluetooth
import DipFFI
import Foundation

/// The radio, and nothing else.
///
/// The core decides everything about who to dial, when, and what to send; this
/// moves bytes and reports what the hardware did. `docs/transport.md#link-layer`
/// and `docs/discovery.md` are the specification, and the two rules that follow
/// from them are:
///
/// - **Dual role, simultaneously.** A `CBPeripheralManager` and a
///   `CBCentralManager` at once, over one service and one characteristic, so a
///   meeting connects whichever way round the two phones happen to be.
/// - **Nothing here is a policy.** The RSSI floor, the six-link cap, the rate
///   limiting and the backoff tiers are `node::scheduler`'s. Reimplementing any
///   of them in Swift is how the two platforms start disagreeing about who is
///   worth dialing.
///
/// Both managers restore state: the system kills and relaunches this app, and
/// the whole design assumes encounters happen with the phone in a pocket.
final class Radio: NSObject {
    /// What the radio reports to, which is the plugin.
    weak var delegate: RadioDelegate?

    private let service = CBUUID(string: DipFFI.serviceUuid())
    private let characteristicId = CBUUID(string: DipFFI.characteristicUuid())

    private var central: CBCentralManager!
    private var peripheral: CBPeripheralManager!

    /// The one characteristic this device serves.
    private var characteristic: CBMutableCharacteristic!

    /// Every live link, both roles, keyed the way the core names them.
    private var links: [UInt64: Link] = [:]

    /// Peripherals seen but not yet dialed, so a `Connect` can name one.
    private var seen: [String: CBPeripheral] = [:]

    /// The next link id. The core keys sessions on these and never reuses one.
    private var nextLink: UInt64 = 1

    /// One connection, from either side.
    private enum Link {
        /// This device dialed, so CoreBluetooth hands us a `CBPeripheral`.
        case dialed(CBPeripheral, CBCharacteristic?)
        /// The peer dialed, so it is a `CBCentral` subscribed to our
        /// characteristic and writes reach us as ATT requests.
        case received(CBCentral)
    }

    override init() {
        super.init()

        central = CBCentralManager(
            delegate: self,
            queue: nil,
            options: [CBCentralManagerOptionRestoreIdentifierKey: "social.coracle.dip.central"])
        peripheral = CBPeripheralManager(
            delegate: self,
            queue: nil,
            options: [CBPeripheralManagerOptionRestoreIdentifierKey: "social.coracle.dip.peripheral"])
    }

    // ------------------------------------------------------------- Actions

    func scan(_ on: Bool) {
        guard central.state == .poweredOn else { return }

        if on {
            // Duplicates off: a sighting is worth reporting once, and the
            // scheduler is what decides when to look again.
            central.scanForPeripherals(
                withServices: [service],
                options: [CBCentralManagerScanOptionAllowDuplicatesKey: false])
        } else {
            central.stopScan()
        }
    }

    func advertise(_ on: Bool) {
        guard peripheral.state == .poweredOn else { return }

        if on {
            // The service UUID and nothing else. In the background iOS strips
            // everything but this anyway — `docs/discovery.md`.
            peripheral.startAdvertising([CBAdvertisementDataServiceUUIDsKey: [service]])
        } else {
            peripheral.stopAdvertising()
        }
    }

    func connect(_ peripheralId: String) {
        guard let target = seen[peripheralId] else { return }

        central.connect(target, options: nil)
    }

    func disconnect(_ link: UInt64) {
        guard case .dialed(let target, _)? = links[link] else {
            links[link] = nil
            return
        }

        central.cancelPeripheralConnection(target)
    }

    /// Write one fragment, already sized to this link's MTU by the core.
    func send(_ link: UInt64, _ fragment: Data) {
        switch links[link] {
        case .dialed(let target, let characteristic?):
            // Acknowledged, because ordered reliable delivery is what the
            // control and sync channels are framed against.
            target.writeValue(fragment, for: characteristic, type: .withResponse)
        case .received(let subscriber):
            // A notify that the queue refused is retried from
            // `peripheralManagerIsReady`, which is the same release the
            // acknowledged path gets.
            if peripheral.updateValue(
                fragment, for: characteristic, onSubscribedCentrals: [subscriber])
            {
                delegate?.radio(self, wroteOn: link)
            } else {
                pending.append((link, fragment))
            }
        default:
            break
        }
    }

    /// Notifies the peripheral queue would not take, oldest first.
    private var pending: [(UInt64, Data)] = []

    // ------------------------------------------------------------ Bookkeeping

    private func take() -> UInt64 {
        defer { nextLink += 1 }

        return nextLink
    }

    private func link(for target: CBPeripheral) -> UInt64? {
        links.first { key, value in
            if case .dialed(let held, _) = value { return held.identifier == target.identifier }
            return false
        }?.key
    }
}

/// What the radio reports. Every one of these is a core entry point.
protocol RadioDelegate: AnyObject {
    func radio(_ radio: Radio, saw peripheral: String, rssi: Int16)
    func radio(_ radio: Radio, upOn link: UInt64, peripheral: String?, dialer: Bool, mtu: UInt32)
    func radio(_ radio: Radio, downOn link: UInt64)
    func radio(_ radio: Radio, received bytes: Data, on link: UInt64)
    func radio(_ radio: Radio, wroteOn link: UInt64)
}

// ------------------------------------------------------------------- Central

extension Radio: CBCentralManagerDelegate {
    func centralManagerDidUpdateState(_ manager: CBCentralManager) {
        // The core asked to scan before the radio was ready, or the user turned
        // Bluetooth back on. Either way the standing request is still standing.
        if manager.state == .poweredOn { scan(true) }
    }

    func centralManager(_ manager: CBCentralManager, willRestoreState state: [String: Any]) {
        // Relaunched into an encounter. The peripherals are back; the sessions
        // are not, so each is torn down and met again from the top.
        let restored = state[CBCentralManagerRestoredStatePeripheralsKey] as? [CBPeripheral] ?? []

        for target in restored {
            manager.cancelPeripheralConnection(target)
        }
    }

    func centralManager(
        _ manager: CBCentralManager,
        didDiscover target: CBPeripheral,
        advertisementData: [String: Any],
        rssi: NSNumber
    ) {
        seen[target.identifier.uuidString] = target

        delegate?.radio(self, saw: target.identifier.uuidString, rssi: Int16(truncating: rssi))
    }

    func centralManager(_ manager: CBCentralManager, didConnect target: CBPeripheral) {
        target.delegate = self
        target.discoverServices([service])
    }

    func centralManager(
        _ manager: CBCentralManager,
        didFailToConnect target: CBPeripheral,
        error: Error?
    ) {
        // Never reported up: a dial that did not complete is not a link, and
        // the scheduler grades it by never hearing about one.
        seen[target.identifier.uuidString] = nil
    }

    func centralManager(
        _ manager: CBCentralManager,
        didDisconnectPeripheral target: CBPeripheral,
        error: Error?
    ) {
        guard let link = link(for: target) else { return }

        links[link] = nil
        delegate?.radio(self, downOn: link)
    }
}

// ---------------------------------------------------------------- Peripheral

extension Radio: CBPeripheralDelegate {
    func peripheral(_ target: CBPeripheral, didDiscoverServices error: Error?) {
        guard let found = target.services?.first(where: { $0.uuid == service }) else {
            return central.cancelPeripheralConnection(target)
        }

        target.discoverCharacteristics([characteristicId], for: found)
    }

    func peripheral(
        _ target: CBPeripheral,
        didDiscoverCharacteristicsFor service: CBService,
        error: Error?
    ) {
        guard let found = service.characteristics?.first(where: { $0.uuid == characteristicId })
        else {
            return central.cancelPeripheralConnection(target)
        }

        // Notify is how the peer writes back on a link this device dialed.
        target.setNotifyValue(true, for: found)

        let link = take()
        links[link] = .dialed(target, found)

        delegate?.radio(
            self,
            upOn: link,
            peripheral: target.identifier.uuidString,
            dialer: true,
            mtu: UInt32(target.maximumWriteValueLength(for: .withResponse)))
    }

    func peripheral(
        _ target: CBPeripheral,
        didUpdateValueFor characteristic: CBCharacteristic,
        error: Error?
    ) {
        guard let link = link(for: target), let value = characteristic.value else { return }

        delegate?.radio(self, received: value, on: link)
    }

    func peripheral(
        _ target: CBPeripheral,
        didWriteValueFor characteristic: CBCharacteristic,
        error: Error?
    ) {
        guard let link = link(for: target) else { return }

        // The acknowledged write is what releases the next fragment. A failed
        // one is still an answer: the core will not stall waiting for it.
        delegate?.radio(self, wroteOn: link)
    }
}

extension Radio: CBPeripheralManagerDelegate {
    func peripheralManagerDidUpdateState(_ manager: CBPeripheralManager) {
        guard manager.state == .poweredOn else { return }

        characteristic = CBMutableCharacteristic(
            type: characteristicId,
            properties: [.notify, .write, .writeWithoutResponse],
            value: nil,
            permissions: [.writeable])

        let served = CBMutableService(type: service, primary: true)
        served.characteristics = [characteristic]

        manager.removeAllServices()
        manager.add(served)
        advertise(true)
    }

    func peripheralManager(_ manager: CBPeripheralManager, willRestoreState state: [String: Any]) {
        // The services come back with the app; the subscribers do not, and a
        // subscriber reappears by writing, which is where the link is made.
        let restored = state[CBPeripheralManagerRestoredStateServicesKey] as? [CBMutableService]

        characteristic =
            restored?
            .first(where: { $0.uuid == service })?
            .characteristics?
            .first(where: { $0.uuid == characteristicId }) as? CBMutableCharacteristic
    }

    func peripheralManager(
        _ manager: CBPeripheralManager,
        central subscriber: CBCentral,
        didSubscribeTo characteristic: CBCharacteristic
    ) {
        let link = take()
        links[link] = .received(subscriber)

        delegate?.radio(
            self,
            upOn: link,
            peripheral: nil,
            dialer: false,
            mtu: UInt32(subscriber.maximumUpdateValueLength))
    }

    func peripheralManager(
        _ manager: CBPeripheralManager,
        central subscriber: CBCentral,
        didUnsubscribeFrom characteristic: CBCharacteristic
    ) {
        guard let link = link(for: subscriber) else { return }

        links[link] = nil
        delegate?.radio(self, downOn: link)
    }

    func peripheralManager(
        _ manager: CBPeripheralManager,
        didReceiveWrite requests: [CBATTRequest]
    ) {
        for request in requests {
            guard let link = link(for: request.central), let value = request.value else { continue }

            delegate?.radio(self, received: value, on: link)
        }

        if let first = requests.first {
            manager.respond(to: first, withResult: .success)
        }
    }

    func peripheralManagerIsReady(toUpdateSubscribers manager: CBPeripheralManager) {
        while let (link, fragment) = pending.first {
            guard case .received(let subscriber)? = links[link] else {
                pending.removeFirst()
                continue
            }
            guard
                manager.updateValue(
                    fragment, for: characteristic, onSubscribedCentrals: [subscriber])
            else { return }

            pending.removeFirst()
            delegate?.radio(self, wroteOn: link)
        }
    }

    private func link(for subscriber: CBCentral) -> UInt64? {
        links.first { key, value in
            if case .received(let held) = value { return held.identifier == subscriber.identifier }
            return false
        }?.key
    }
}
