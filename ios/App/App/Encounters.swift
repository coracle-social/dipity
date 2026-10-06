import Capacitor
import Foundation

/// What only a screen can do with what the core says.
protocol EncountersView: AnyObject {
    /// Hand the view one notification.
    func notify(_ event: String, _ data: [String: Any])

    /// Offer the backup file, answering whether there was anywhere to offer it.
    func share(_ file: URL) -> Bool
}

/// The core for as long as the process lives, whether or not a screen shows it.
///
/// Gossip happens with both phones in pockets, and iOS relaunches the app into
/// the background to deliver a Bluetooth event with no scene and no webview, so
/// the store, the node, the radio and the lifecycle live here. The app delegate
/// creates this at every launch, which is what restores the radio's managers,
/// and opens the core whenever there is an identity to open it under. The
/// plugin attaches as the view while there is one.
///
/// What only a screen wants — a store change, a pairing request, a transfer
/// prompt — goes to the attached view and is dropped when there is none. A view
/// that attaches later re-reads everything on `start`. Everything here runs on
/// the main queue, which is where the radio's managers and the timers deliver.
/// `docs/overview.md#architecture`.
///
/// # The loop
///
/// Every core entry point answers a list of `Action`, and `apply` is the one
/// place they are carried out. The core calls nothing back but custody, so this
/// is never re-entered from inside a call: an action that produces more actions
/// produces them on the next entry point, not underneath this one.
final class Encounters {
    /// The one per process.
    static let shared = Encounters()

    /// The store and the node, which are opened together and dropped together.
    struct Core {
        let store: Store
        let node: Node
    }

    private let radio = Radio()
    private var lifecycle: Lifecycle?

    /// The core, once there is an identity to open it under.
    private(set) var core: Core?

    /// The store registration, live until the core closes.
    private var watching: Subscription?

    /// The screen, while there is one.
    private weak var view: EncountersView?

    private init() {
        OsLog.install()
        radio.delegate = self
    }

    /// The plugin, while it exists.
    func attach(_ view: EncountersView) {
        self.view = view
    }

    /// Open the core at launch if there is an identity: a launch into the
    /// background to deliver a Bluetooth event has nothing else that would.
    func launch() {
        guard Keychain.has() else { return }

        do {
            _ = try open()
        } catch {
            CAPLog.print("dip: the core could not be opened at launch: \(error)")
        }
    }

    /// Open the core if it is not open, and answer the identity it runs as.
    func open() throws -> String {
        if let core { return try core.node.identity() }

        let directory = try support()
        let store = try Store.open(directory: directory.path)
        let node = try Node.open(
            store: store,
            custody: KeychainCustody(),
            directory: directory.path)

        core = Core(store: store, node: node)

        // Registered before the first tick, so nothing the core does on the way up goes unheard.
        watching = store.observe(observer: StoreChanges())

        lifecycle = Lifecycle(
            foregrounded: { [weak self] in
                Alerts.clear()
                self?.drive { try $0.notifyForegrounded() }
            },
            backgrounded: { [weak self] in self?.drive { try $0.notifyBackgrounded() } },
            battery: { [weak self] level in self?.drive { try $0.battery(level: level) } },
            tick: { [weak self] in self?.drive { try $0.tick() } })

        // Where the app is and what the battery is at, before anything decides on either.
        lifecycle?.report()

        // Nothing is scanning or advertising until the core says so, and it says so on the first tick.
        apply(try node.tick())

        return try node.identity()
    }

    /// Drop the node and everything driving it, so `open` can run again.
    ///
    /// The store observer and the lifecycle go with their references; the radio
    /// keeps its GATT service, which is registered once at power-on and would
    /// not come back on its own.
    func close() {
        watching = nil
        lifecycle = nil
        radio.stop()
        core = nil
    }

    /// Run one core entry point nobody is waiting on, and carry out what it
    /// answered.
    ///
    /// A `NodeError.Link` is the core refusing to carry on with that link, so
    /// it goes; anything else is logged and the loop continues.
    func drive(_ call: (Node) throws -> [Action]) {
        guard let node = core?.node else { return }

        do {
            apply(try call(node))
        } catch let error as NodeError {
            if case .Link(let link, _) = error { radio.disconnect(link.value) }

            CAPLog.print("dip: \(error)")
        } catch {
            CAPLog.print("dip: \(error)")
        }
    }

    /// Carry out what the core asked for, in the order it asked.
    func apply(_ actions: [Action]) {
        for action in actions {
            switch action {
            case .scan(let on):
                radio.scan(on)
            case .advertise(let on):
                radio.advertise(on)
            case .connect(let peripheral):
                radio.connect(peripheral.value)
            case .waitFor(let peripheral):
                radio.waitFor(peripheral.value)
            case .stopWaiting(let peripheral):
                radio.stopWaiting(peripheral.value)
            case .disconnect(let link):
                // The view hears about a link the core ended as it does about one the radio lost.
                radio.disconnect(link.value)
                notify("linkClosed", ["link": Int(link.value)])
            case .send(let link, let fragment):
                radio.send(link.value, fragment)
            case .requestApproval(let link, let code):
                notify("requestApproval", ["link": Int(link.value), "code": Int(code)])
            case .peerIdentified(let link, let pubkey, let code, let dialed):
                notify(
                    "peerIdentified",
                    ["link": Int(link.value), "pubkey": pubkey, "code": Int(code), "dialed": dialed])
            case .confirmIdentityTransfer(let link, let code):
                notify("confirmIdentityTransfer", ["link": Int(link.value), "code": Int(code)])
            case .identityTransfer(let link, let outcome):
                notify(
                    "identityTransfer",
                    ["link": Int(link.value), "outcome": App.outcomeName(outcome: outcome)])
            case .shareKeyBackup(let path):
                // A backup with nowhere to go is deleted rather than left on disk.
                if view?.share(URL(fileURLWithPath: path)) != true {
                    drive { try $0.keyExportFinished() }
                }
            case .notify(let announcement):
                Alerts.post(announcement)
            case .wakeAt(let at):
                // Advisory: a suspended app runs no timer, and the next radio callback covers it.
                lifecycle?.wake(at: at)
            case .sendBulk(let link, let fragment):
                radio.sendBulk(link.value, fragment)
            case .publishL2cap(let link):
                radio.publishL2cap(link.value)
            case .openL2cap(let link, let psm):
                radio.openL2cap(link.value, psm)
            }
        }
    }

    /// Whether Bluetooth can be used, which the radio knows from launch on.
    var power: String { radio.power }

    /// Hand a notification to the view, if there is one to hear it.
    func notify(_ event: String, _ data: [String: Any]) {
        view?.notify(event, data)
    }

    /// Where the store and the blobs live: a directory in Application Support,
    /// which is not purged, unlike Caches, and kept out of backups, because
    /// `event_seen` is a record of who the user was near.
    /// `docs/storage.md#the-sqlite-store`.
    private func support() throws -> URL {
        let base = try FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true)
        var directory = base.appendingPathComponent("dip", isDirectory: true)

        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

        var excluded = URLResourceValues()
        excluded.isExcludedFromBackup = true
        try directory.setResourceValues(excluded)

        return directory
    }
}

// --------------------------------------------------------------------- Store

/// Tells the view which group of tables moved, so it re-reads what it shows.
///
/// Store changes arrive on the store's own thread, so they hop to the main
/// queue the view lives on.
private final class StoreChanges: StoreObserver {
    func changed(group: Change) {
        let name = App.changeName(group: group)

        DispatchQueue.main.async {
            Encounters.shared.notify("storeChanged", ["group": name])
        }
    }
}

// --------------------------------------------------------------------- Radio

extension Encounters: RadioDelegate {
    func radio(_ radio: Radio, saw peripheral: String, rssi: Int16) {
        drive { try $0.peripheralSeen(peripheral: PeripheralId(value: peripheral), rssi: rssi) }
    }

    func radio(_ radio: Radio, dialFailed peripheral: String) {
        drive { try $0.dialFailed(peripheral: PeripheralId(value: peripheral)) }
    }

    func radio(_ radio: Radio, notOurs peripheral: String) {
        drive { try $0.notOurs(peripheral: PeripheralId(value: peripheral)) }
    }

    func radio(_ radio: Radio, upOn link: UInt64, peripheral: String?, dialer: Bool, mtu: UInt32) {
        drive {
            try $0.linkUp(
                link: LinkId(value: link),
                peripheral: peripheral.map { PeripheralId(value: $0) },
                role: dialer ? .dialer : .receiver,
                mtu: mtu)
        }
    }

    // The view is told too: a screen naming a link cannot offer over a dead one.
    func radio(_ radio: Radio, downOn link: UInt64) {
        notify("linkClosed", ["link": Int(link)])

        drive { try $0.linkDown(link: LinkId(value: link)) }
    }

    func radio(_ radio: Radio, received bytes: Data, on link: UInt64) {
        drive { try $0.bytesReceived(link: LinkId(value: link), write: bytes) }
    }

    func radio(_ radio: Radio, wroteOn link: UInt64) {
        drive { try $0.writeComplete(link: LinkId(value: link)) }
    }

    func radio(_ radio: Radio, publishedOn link: UInt64, psm: UInt16) {
        drive { try $0.l2capPublished(link: LinkId(value: link), psm: psm) }
    }

    func radio(_ radio: Radio, bulkUpOn link: UInt64, mtu: UInt32) {
        drive { try $0.l2capOpened(link: LinkId(value: link), mtu: mtu) }
    }

    func radio(_ radio: Radio, bulkDownOn link: UInt64) {
        drive { try $0.l2capUnavailable(link: LinkId(value: link)) }
    }

    func radio(_ radio: Radio, receivedBulk bytes: Data, on link: UInt64) {
        drive { try $0.bulkReceived(link: LinkId(value: link), read: bytes) }
    }

    func radio(_ radio: Radio, wroteBulkOn link: UInt64) {
        drive { try $0.bulkWriteComplete(link: LinkId(value: link)) }
    }

    // Nothing to tell the core: it asks to scan regardless, and the radio keeps the request standing.
    func radio(_ radio: Radio, power: String) {
        notify("bluetooth", ["state": power])
    }
}
