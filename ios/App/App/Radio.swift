import CoreBluetooth
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
///
/// The main queue owns the state: `links`, `seen`, `bulk`, `publishing`,
/// `published`, `pending` and `nextLink` are read and written there and none of
/// them is locked. Nothing enforces that — it is what `queue: nil` means on both
/// managers, plus `BulkChannel` scheduling its streams on `.main`, so a delegate
/// built with a queue of its own would break every one of them with no compiler
/// complaint. `Radio.kt` has the same rule and has to say so with a
/// single-thread executor.
final class Radio: NSObject {
    /// What the radio reports to, which is the plugin.
    weak var delegate: RadioDelegate?

    private let service = CBUUID(string: App.serviceUuid())
    private let characteristicId = CBUUID(string: App.characteristicUuid())

    private var central: CBCentralManager!
    private var peripheral: CBPeripheralManager!

    /// The one characteristic this device serves.
    private var characteristic: CBMutableCharacteristic!

    /// What the core last asked of each manager, applied again whenever the
    /// manager powers on.
    private var scanning = false
    private var advertising = false

    /// Every live link, both roles, keyed the way the core names them.
    private var links: [UInt64: Link] = [:]

    /// Peripherals seen, so a `Connect` can name one, including a redial.
    private var seen: [String: CBPeripheral] = [:]

    /// When each peripheral was last reported, so one advertising many times a
    /// second is reported once a second.
    private var reported: [String: Date] = [:]

    /// The open L2CAP channel on each link, for as long as one is.
    private var bulk: [UInt64: BulkChannel] = [:]

    /// Links that have asked the peripheral manager for a channel, oldest
    /// first.
    ///
    /// `publishL2CAPChannel` names no link and its callback answers with a PSM
    /// and nothing else, so the order they were asked in is the only thing
    /// tying a PSM back to whoever wanted it.
    private var publishing: [UInt64] = []

    /// The PSM each publishing link is waiting for its peer at.
    private var published: [UInt64: CBL2CAPPSM] = [:]

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

        // `queue: nil` is the main queue, and that is the whole of the
        // confinement above: a queue here is every field of this class locked.
        central = CBCentralManager(
            delegate: self,
            queue: nil,
            options: [CBCentralManagerOptionRestoreIdentifierKey: "social.coracle.dipity.central"])
        peripheral = CBPeripheralManager(
            delegate: self,
            queue: nil,
            options: [CBPeripheralManagerOptionRestoreIdentifierKey: "social.coracle.dipity.peripheral"])
    }

    // ------------------------------------------------------------- Actions

    func scan(_ on: Bool) {
        scanning = on
        guard central.state == .poweredOn else { return }

        if on {
            // Duplicates on, which iOS honours only in the foreground: a peer the scheduler queued keeps being seen.
            central.scanForPeripherals(
                withServices: [service],
                options: [CBCentralManagerScanOptionAllowDuplicatesKey: true])
        } else {
            central.stopScan()
        }
    }

    func advertise(_ on: Bool) {
        advertising = on
        guard peripheral.state == .poweredOn, characteristic != nil else { return }

        if on {
            // The service UUID and nothing else. In the background iOS strips
            // everything but this anyway — `docs/discovery.md`.
            peripheral.startAdvertising([CBAdvertisementDataServiceUUIDsKey: [service]])
        } else {
            peripheral.stopAdvertising()
        }
    }

    func connect(_ peripheralId: String) {
        let known = UUID(uuidString: peripheralId).flatMap {
            central.retrievePeripherals(withIdentifiers: [$0]).first
        }

        guard let target = seen[peripheralId] ?? known else { return }

        central.connect(target, options: nil)
    }

    func disconnect(_ link: UInt64) {
        guard case .dialed(let target, _)? = links[link] else {
            return forget(link)
        }

        central.cancelPeripheralConnection(target)
    }

    /// Publish a channel for the peer on `link` to connect to.
    ///
    /// Only a link the peer dialed can: publishing is the peripheral manager's,
    /// which is the GATT role this device holds on one it received.
    func publishL2cap(_ link: UInt64) {
        guard case .received(_)? = links[link], peripheral.state == .poweredOn else {
            return bulkUnavailable(link)
        }

        publishing.append(link)
        peripheral.publishL2CAPChannel(withEncryption: false)
    }

    /// Open the channel the peer published at `psm`.
    func openL2cap(_ link: UInt64, _ psm: UInt16) {
        guard case .dialed(let target, _)? = links[link] else {
            return bulkUnavailable(link)
        }

        target.openL2CAPChannel(CBL2CAPPSM(psm))
    }

    /// Write one bulk fragment, already sized and length-prefixed by the core.
    func sendBulk(_ link: UInt64, _ fragment: Data) {
        guard let channel = bulk[link] else {
            return bulkUnavailable(link)
        }

        channel.write(fragment)
    }

    /// Stop both roles and drop every link.
    ///
    /// The GATT service stays registered, unlike Android's, because it is added
    /// from `peripheralManagerDidUpdateState` and that fires once at power-on.
    /// Whether Bluetooth can be used, as the view words it: `on`, `off`, `denied`,
    /// `unsupported`, or `unknown` while the system has not said yet.
    var power: String {
        switch central.state {
        case .poweredOn: return "on"
        case .poweredOff: return "off"
        case .unauthorized: return "denied"
        case .unsupported: return "unsupported"
        default: return "unknown"
        }
    }

    func stop() {
        scan(false)
        advertise(false)
        Array(links.keys).forEach(disconnect)
        Array(bulk.keys).forEach(forget)
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

    /// Drop a link and everything hanging off it.
    ///
    /// The link goes first, so the channel closing on its way out is not
    /// reported as an upgrade this device lost.
    /// A pending publication is left in the queue rather than dropped: the
    /// order is what pairs a PSM with whoever asked for it, and the callback
    /// unpublishes one whose link has gone.
    private func forget(_ link: UInt64) {
        links[link] = nil
        bulk.removeValue(forKey: link)?.close()

        if let psm = published.removeValue(forKey: link) {
            peripheral.unpublishL2CAPChannel(psm)
        }
    }

    /// Tell the core this link finishes on GATT.
    private func bulkUnavailable(_ link: UInt64) {
        delegate?.radio(self, bulkDownOn: link)
    }

    /// Take an open channel over, and tell the core bulk can move onto it.
    private func adopt(_ channel: CBL2CAPChannel, on link: UInt64) {
        let pump = BulkChannel(channel, on: link)

        pump.delegate = self
        bulk[link] = pump
        pump.open()

        delegate?.radio(self, bulkUpOn: link, mtu: Self.bulkMtu)
    }

    /// What one bulk write carries, length prefix included.
    ///
    /// CoreBluetooth negotiates the channel's MTU and exposes it nowhere, and
    /// the streams take a write of any size, so this is the size the core cuts
    /// fragments to rather than a ceiling the radio was given.
    private static let bulkMtu: UInt32 = 8192
}

/// What the radio reports. Every one of these is a core entry point.
protocol RadioDelegate: AnyObject {
    func radio(_ radio: Radio, saw peripheral: String, rssi: Int16)
    func radio(_ radio: Radio, dialFailed peripheral: String)
    func radio(_ radio: Radio, upOn link: UInt64, peripheral: String?, dialer: Bool, mtu: UInt32)
    func radio(_ radio: Radio, downOn link: UInt64)
    func radio(_ radio: Radio, received bytes: Data, on link: UInt64)
    func radio(_ radio: Radio, wroteOn link: UInt64)
    func radio(_ radio: Radio, publishedOn link: UInt64, psm: UInt16)
    func radio(_ radio: Radio, bulkUpOn link: UInt64, mtu: UInt32)
    func radio(_ radio: Radio, bulkDownOn link: UInt64)
    func radio(_ radio: Radio, receivedBulk bytes: Data, on link: UInt64)
    func radio(_ radio: Radio, wroteBulkOn link: UInt64)
    func radio(_ radio: Radio, power: String)
}

// ------------------------------------------------------------------- Central

extension Radio: CBCentralManagerDelegate {
    func centralManagerDidUpdateState(_ manager: CBCentralManager) {
        // The core asked to scan before the radio was ready, or the user turned
        // Bluetooth back on. Either way the standing request is still standing.
        if manager.state == .poweredOn { scan(scanning) }

        delegate?.radio(self, power: power)
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
        let id = target.identifier.uuidString
        let now = Date()

        seen[id] = target

        if let last = reported[id], now.timeIntervalSince(last) < 1 { return }

        reported[id] = now
        delegate?.radio(self, saw: id, rssi: Int16(truncating: rssi))
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
        // Not a link, but the scheduler retries a failed dial shortly rather than after a never-answered minute.
        delegate?.radio(self, dialFailed: target.identifier.uuidString)
    }

    func centralManager(
        _ manager: CBCentralManager,
        didDisconnectPeripheral target: CBPeripheral,
        error: Error?
    ) {
        // A dial that connected but never became a link, its service missing or its subscription refused.
        guard let link = link(for: target) else {
            delegate?.radio(self, dialFailed: target.identifier.uuidString)
            return
        }

        forget(link)
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
            // The single-write size: `.withResponse` answers 512 whatever the ATT MTU, which is a long write.
            mtu: UInt32(target.maximumWriteValueLength(for: .withoutResponse)))
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

    func peripheral(_ target: CBPeripheral, didOpen channel: CBL2CAPChannel?, error: Error?) {
        guard let link = link(for: target) else { return }
        guard let channel, error == nil else {
            return bulkUnavailable(link)
        }

        adopt(channel, on: link)
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
        advertise(advertising)
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

        forget(link)
        delegate?.radio(self, downOn: link)
    }

    func peripheralManager(
        _ manager: CBPeripheralManager,
        didReceiveWrite requests: [CBATTRequest]
    ) {
        guard let first = requests.first else { return }

        // A long write splits one fragment across requests, which the core would read as several.
        guard requests.allSatisfy({ $0.offset == 0 }) else {
            return manager.respond(to: first, withResult: .invalidOffset)
        }

        for request in requests {
            guard let link = link(for: request.central), let value = request.value else { continue }

            delegate?.radio(self, received: value, on: link)
        }

        manager.respond(to: first, withResult: .success)
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

    func peripheralManager(
        _ manager: CBPeripheralManager,
        didPublishL2CAPChannel psm: CBL2CAPPSM,
        error: Error?
    ) {
        guard !publishing.isEmpty else { return }

        let link = publishing.removeFirst()

        guard error == nil else {
            return bulkUnavailable(link)
        }

        // The link may have gone while the manager was assigning a PSM, and a
        // channel nobody is waiting at stays published until it is taken back.
        guard links[link] != nil else {
            return manager.unpublishL2CAPChannel(psm)
        }

        published[link] = psm
        delegate?.radio(self, publishedOn: link, psm: UInt16(psm))
    }

    func peripheralManager(
        _ manager: CBPeripheralManager,
        didOpen channel: CBL2CAPChannel?,
        error: Error?
    ) {
        guard let channel, let link = link(publishedAt: channel.psm) else { return }

        adopt(channel, on: link)
    }

    private func link(for subscriber: CBCentral) -> UInt64? {
        links.first { key, value in
            if case .received(let held) = value { return held.identifier == subscriber.identifier }
            return false
        }?.key
    }

    /// Whoever the PSM was published for, which is what ties an inbound channel
    /// to a link: the peer arrives as a `CBCentral` the GATT one never was.
    private func link(publishedAt psm: CBL2CAPPSM) -> UInt64? {
        published.first { _, at in at == psm }?.key
    }
}

// ---------------------------------------------------------------------- Bulk

extension Radio: BulkDelegate {
    func bulkRead(_ channel: BulkChannel, bytes: Data) {
        delegate?.radio(self, receivedBulk: bytes, on: channel.link)
    }

    func bulkWrote(_ channel: BulkChannel) {
        delegate?.radio(self, wroteBulkOn: channel.link)
    }

    /// The channel went away and this link finishes on GATT, which the core is
    /// told by the same path a failure to open one takes.
    func bulkClosed(_ channel: BulkChannel) {
        guard bulk[channel.link] === channel else { return }

        bulk[channel.link] = nil

        if let psm = published.removeValue(forKey: channel.link) {
            peripheral.unpublishL2CAPChannel(psm)
        }

        bulkUnavailable(channel.link)
    }
}
