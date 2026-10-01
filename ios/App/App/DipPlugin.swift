import Capacitor
import Foundation
import LocalAuthentication
import UIKit

/// The webview's end of the core.
///
/// One plugin, holding the store, the node and the radio, with a method per
/// call the view makes. Everything below the bridge is `dip_ffi`; nothing here
/// decides anything about gossip, the radio schedule or policy.
///
/// # The loop
///
/// Every core entry point answers a list of `Action`, and `apply` is the one
/// place they are carried out. The core calls nothing back, so this is never
/// re-entered from inside a call — an action that produces more actions
/// produces them on the next entry point, not underneath this one.
///
/// # The three call shapes
///
/// A store read answers what it read, through `answer`. A node call answers
/// only the actions it asked for, through `perform`. A radio event has nobody
/// waiting on it, through `drive`. Each takes the opened core, runs one entry
/// point and carries out the result; a method that spells any of that out again
/// is a method doing something the other two are not.
///
/// # The main queue
///
/// Capacitor calls plugin methods on its own bridge queue, while the radio, the
/// lifecycle timers and `core` itself belong to the main queue. Every method
/// that touches any of them hops there first, through `onMain`.
@objc(DipPlugin)
public class DipPlugin: CAPPlugin, CAPBridgedPlugin {
    public let identifier = "DipPlugin"
    public let jsName = "Dip"
    public let pluginMethods: [CAPPluginMethod] = [
        CAPPluginMethod(name: "coreVersion", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "hasIdentity", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "createIdentity", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "importIdentity", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "deleteIdentity", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "start", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "mediaTags", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "publish", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "approve", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "exportKey", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "offerIdentity", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "answerIdentityTransfer", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "takeTransferredIdentity", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "listEvents", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "listDetails", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "getEvent", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "forgetEvent", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "wantedBlobs", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "getBlob", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "eventsReferencingBlob", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "policy", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "preferences", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "preference", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "setPreference", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "clearPreference", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "setCanGoBack", returnType: CAPPluginReturnPromise),
    ]

    /// The store and the node, which are opened together and dropped together.
    ///
    /// One field rather than two: every call the view makes needs one or both,
    /// and a plugin holding half a core is a state no call site should have to
    /// consider.
    struct Core {
        let store: Store
        let node: Node
    }

    private let radio = Radio()
    private var lifecycle: Lifecycle?

    /// The `exportKey` call waiting on the sheet it opened.
    private var exporting: CAPPluginCall?

    /// The core, once there is an identity to open it under.
    private var core: Core?

    /// The store registration, live until the plugin drops it.
    private var watching: Subscription?

    /// Where the core's log goes, installed before anything can log.
    public override func load() {
        OsLog.install()
    }

    // ---------------------------------------------------------------- Identity

    @objc func coreVersion(_ call: CAPPluginCall) {
        call.resolve(["version": App.coreVersion()])
    }

    @objc func hasIdentity(_ call: CAPPluginCall) {
        call.resolve(["exists": Keychain.has()])
    }

    @objc func createIdentity(_ call: CAPPluginCall) {
        adopt(App.generateIdentity(), into: call)
    }

    @objc func importIdentity(_ call: CAPPluginCall) {
        guard let nsec = call.getString("nsec") else {
            return call.reject("importIdentity needs an nsec")
        }

        do {
            adopt(try App.identityFromNsec(nsec: nsec), into: call)
        } catch {
            call.reject("that is not an nsec", nil, error)
        }
    }

    @objc func deleteIdentity(_ call: CAPPluginCall) {
        call.resolve(["existed": Keychain.delete()])
    }

    // ------------------------------------------------------------------- Node

    /// Open the store and the node, and start the radio.
    ///
    /// The view calls this once it knows there is an identity, which is what
    /// makes first run a screen rather than a failed open.
    @objc func start(_ call: CAPPluginCall) {
        onMain {
            guard self.core == nil else { return call.resolve() }

            self.open(into: call)
        }
    }

    private func open(into call: CAPPluginCall) {
        do {
            let directory = try support()
            let store = try Store.open(directory: directory.path)
            let node = try Node.open(
                store: store,
                custody: KeychainCustody(),
                directory: directory.path)

            core = Core(store: store, node: node)

            // Registered before the first tick, so nothing the core does on the
            // way up is a change the view never hears about.
            watching = store.observe(observer: StoreChanges(self))

            radio.delegate = self
            lifecycle = Lifecycle(
                foregrounded: { [weak self] in self?.drive { try $0.notifyForegrounded() } },
                backgrounded: { [weak self] in self?.drive { try $0.notifyBackgrounded() } },
                battery: { [weak self] level in self?.drive { try $0.battery(level: level) } },
                tick: { [weak self] in self?.drive { try $0.tick() } })

            // Where the app is and what the battery is at, before anything
            // decides on either.
            lifecycle?.report()

            // Nothing is scanning or advertising until the core says so, and it
            // says so on the first tick.
            apply(try node.tick())
            call.resolve(["identity": try node.identity()])
        } catch {
            call.reject("the core could not be opened", nil, error)
        }
    }

    /// The `imeta` entries an event has to carry for a peer to fetch `media`
    /// and check what it gets, base64 in.
    ///
    /// Describing bytes stores nothing, so this is the one node call needing no
    /// started core: the view composes the tag, signs the event, and hands both
    /// to `publish`. `docs/storage.md#blob-store`.
    @objc func mediaTags(_ call: CAPPluginCall) {
        guard let media = call.getString("media").flatMap({ Data(base64Encoded: $0) }) else {
            return call.reject("mediaTags needs base64 media")
        }

        call.resolve(["entries": App.mediaTags(bytes: media)])
    }

    /// Store and offer an event the view built, with the media it attaches.
    @objc func publish(_ call: CAPPluginCall) {
        guard let event = call.getString("event") else {
            return call.reject("publish needs an event")
        }

        let entries = call.getArray("media", String.self) ?? []
        let media = entries.compactMap { Data(base64Encoded: $0) }

        // Dropping one would publish an event whose `imeta` names media this
        // device never stored, and the author would land on its own want list.
        guard media.count == entries.count else {
            return call.reject("publish needs base64 media")
        }

        perform(call, "that event could not be published") {
            try $0.node.publish(event: event, media: media)
        }
    }

    /// The user answered a `requestApproval` the plugin sent up.
    @objc func approve(_ call: CAPPluginCall) {
        guard let link = link(call) else { return call.reject("approve needs a link") }

        perform(call, "that approval could not be recorded") {
            try $0.node.approve(link: link, approved: call.getBool("approved", false))
        }
    }

    /// Write a key backup into the cache directory and ask for the share sheet.
    ///
    /// The path never comes back over the bridge: the view learns only that the
    /// file was shared or dismissed. Answers once the sheet closes, which is
    /// also when the file goes. `docs/keys.md#backup`.
    @objc func exportKey(_ call: CAPPluginCall) {
        onMain {
            self.confirmOwner("Save a copy of your key", or: call) { self.export(into: call) }
        }
    }

    private func export(into call: CAPPluginCall) {
        guard let core else { return call.reject("exportKey needs a started core") }

        // One sheet means one call waiting on it, so whoever this displaces is
        // answered rather than left on a promise that never settles.
        exporting?.reject("another key export replaced this one")
        exporting = call

        do {
            apply(
                try core.node.exportKey(
                    cache: FileManager.default.temporaryDirectory.path,
                    password: call.getString("password")))
        } catch {
            exporting = nil
            call.reject("the backup could not be written", nil, error)
        }
    }

    // ------------------------------------------------- Login with device

    /// Offer this device's identity to the peer on `link`.
    ///
    /// Both ends are asked to compare the six digits a `confirmIdentityTransfer`
    /// carries before anything moves. `docs/keys.md#login-with-device`.
    @objc func offerIdentity(_ call: CAPPluginCall) {
        guard let link = link(call) else { return call.reject("offerIdentity needs a link") }

        onMain {
            self.confirmOwner("Put your key on another phone", or: call) {
                self.perform(call, "that identity could not be offered") {
                    try $0.node.offerIdentity(link: link)
                }
            }
        }
    }

    /// The user answered a `confirmIdentityTransfer` the plugin sent up.
    @objc func answerIdentityTransfer(_ call: CAPPluginCall) {
        guard let link = link(call) else {
            return call.reject("answerIdentityTransfer needs a link")
        }

        perform(call, "that answer could not be recorded") {
            try $0.node.answerIdentityTransfer(
                link: link, confirmed: call.getBool("confirmed", false))
        }
    }

    /// Adopt the identity an `identityTransfer` of `received` announced.
    ///
    /// The key is handed out once and does not cross the bridge: it is written
    /// to the Keychain here, the same custody path a generated one takes, and
    /// the node is reopened under it. Answers what `start` answers, so the view
    /// reads the new identity off the same field.
    @objc func takeTransferredIdentity(_ call: CAPPluginCall) {
        onMain { self.adoptTransferred(into: call) }
    }

    private func adoptTransferred(into call: CAPPluginCall) {
        guard let core else { return call.reject("takeTransferredIdentity needs a started core") }
        guard let link = link(call) else {
            return call.reject("takeTransferredIdentity needs a link")
        }

        do {
            guard let secret = try core.node.takeTransferredIdentity(link: link) else {
                return call.reject("no identity arrived on that link")
            }

            try Keychain.write(secret)
            close()
            open(into: call)
        } catch {
            call.reject("the transferred identity could not be adopted", nil, error)
        }
    }

    // ------------------------------------------------------------------ Back

    /// Nothing: iOS has no back button, and the view asks both shells the same.
    @objc func setCanGoBack(_ call: CAPPluginCall) {
        call.resolve()
    }

    // ----------------------------------------------------------------- Store

    @objc func listEvents(_ call: CAPPluginCall) {
        answer(call, "events") { try $0.listEvents(query: self.query(call)) }
    }

    @objc func listDetails(_ call: CAPPluginCall) {
        answer(call, "details") { try $0.listDetails(query: self.query(call)) }
    }

    @objc func getEvent(_ call: CAPPluginCall) {
        guard let id = call.getString("id") else { return call.reject("getEvent needs an id") }

        answer(call, "event") { try $0.getEvent(id: id) ?? NSNull() }
    }

    /// Drop one event from this device. Local and silent: no peer is told.
    @objc func forgetEvent(_ call: CAPPluginCall) {
        guard let id = call.getString("id") else { return call.reject("forgetEvent needs an id") }

        answer(call, "existed") { try $0.forgetEvent(id: id) }
    }

    @objc func wantedBlobs(_ call: CAPPluginCall) {
        let limit = UInt32(call.getInt("limit") ?? 32)

        answer(call, "blobs") { try $0.wantedBlobs(limit: limit) }
    }

    @objc func getBlob(_ call: CAPPluginCall) {
        guard let sha256 = call.getString("sha256") else {
            return call.reject("getBlob needs a sha256")
        }

        answer(call, "blob") { try $0.getBlob(sha256: sha256) ?? NSNull() }
    }

    @objc func eventsReferencingBlob(_ call: CAPPluginCall) {
        guard let sha256 = call.getString("sha256") else {
            return call.reject("eventsReferencingBlob needs a sha256")
        }

        answer(call, "ids") { try $0.eventsReferencingBlob(sha256: sha256) }
    }

    /// The compiled policy every live session is bound to, as JSON, with the
    /// core's own defaults where nothing is written.
    @objc func policy(_ call: CAPPluginCall) {
        onMain {
            guard let core = self.core else { return call.reject("policy needs a started core") }

            do {
                call.resolve(["policy": try core.node.policy()])
            } catch {
                call.reject("the policy could not be read", nil, error)
            }
        }
    }

    @objc func preferences(_ call: CAPPluginCall) {
        answer(call, "preferences") {
            try $0.preferences().map {
                ["key": $0.key, "value": $0.value, "updatedAt": Int($0.updatedAt)]
            }
        }
    }

    @objc func preference(_ call: CAPPluginCall) {
        guard let key = call.getString("key") else { return call.reject("preference needs a key") }

        answer(call, "value") { try $0.preference(key: key) ?? NSNull() }
    }

    /// Write a preference and rebind live sessions under the policy it compiles
    /// to, which is why this is one call rather than two the view can misorder.
    @objc func setPreference(_ call: CAPPluginCall) {
        guard let key = call.getString("key"), let value = call.getString("value") else {
            return call.reject("setPreference needs a key and a value")
        }

        perform(call, "that preference could not be written") {
            try $0.store.setPreference(key: key, value: value)

            return try $0.node.policyChanged()
        }
    }

    @objc func clearPreference(_ call: CAPPluginCall) {
        guard let key = call.getString("key") else {
            return call.reject("clearPreference needs a key")
        }

        onMain {
            guard let core = self.core else {
                return call.reject("clearPreference needs a started core")
            }

            do {
                let existed = try core.store.clearPreference(key: key)

                self.apply(try core.node.policyChanged())
                call.resolve(["existed": existed])
            } catch {
                call.reject("that preference could not be cleared", nil, error)
            }
        }
    }

    /// Run one store read and answer what it gave back under `key`.
    private func answer(
        _ call: CAPPluginCall, _ key: String, _ read: @escaping (Store) throws -> Any
    ) {
        onMain {
            guard let core = self.core else { return call.reject("that call needs a started core") }

            do {
                call.resolve([key: try read(core.store)])
            } catch {
                call.reject("the store could not answer", nil, error)
            }
        }
    }

    /// Run one core entry point the view is waiting on, and answer when the
    /// actions it asked for have been carried out.
    ///
    /// The other half of `answer`: a store read answers with what it read, and
    /// a node call answers with nothing.
    private func perform(
        _ call: CAPPluginCall, _ failure: String, _ body: @escaping (Core) throws -> [Action]
    ) {
        onMain {
            guard let core = self.core else { return call.reject("that call needs a started core") }

            do {
                self.apply(try body(core))
                call.resolve()
            } catch {
                call.reject(failure, nil, error)
            }
        }
    }

    /// Ask whoever holds the phone to prove they own it before the key leaves,
    /// rejecting `call` if they do not. A phone with no passcode has nothing to
    /// ask and goes straight through. `docs/keys.md#backup`.
    private func confirmOwner(
        _ reason: String, or call: CAPPluginCall, then: @escaping () -> Void
    ) {
        let context = LAContext()
        var unavailable: NSError?

        guard context.canEvaluatePolicy(.deviceOwnerAuthentication, error: &unavailable) else {
            return then()
        }

        context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason) { confirmed, _ in
            DispatchQueue.main.async {
                confirmed ? then() : call.reject("the owner did not confirm")
            }
        }
    }

    /// Run `work` on the main queue, which owns the radio, the timers and `core`.
    private func onMain(_ work: @escaping () -> Void) {
        if Thread.isMainThread { work() } else { DispatchQueue.main.async(execute: work) }
    }

    /// The link the view named, which it only ever learned by being asked
    /// something about it.
    ///
    /// A link is a `UInt64` the shell assigned, so a negative one is not a link
    /// this device ever handed out — refused here rather than trapping on the
    /// conversion.
    private func link(_ call: CAPPluginCall) -> LinkId? {
        guard let value = call.getInt("link"), value >= 0 else { return nil }

        return LinkId(value: UInt64(value))
    }

    /// The query the view named, which is a filter plus provenance it may not
    /// smuggle onto one.
    private func query(_ call: CAPPluginCall) -> Query {
        Query(
            filter: call.getString("filter"),
            seenSince: call.getInt("seenSince").map(Int64.init),
            seenUntil: call.getInt("seenUntil").map(Int64.init),
            seenFrom: call.getArray("seenFrom", String.self),
            order: call.getString("order") == "seenAt" ? .seenAt : .createdAt)
    }

    // ---------------------------------------------------------------- Actions

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
            case .disconnect(let link):
                radio.disconnect(link.value)
            case .send(let link, let fragment):
                radio.send(link.value, fragment)
            case .requestApproval(let link, let code):
                notifyListeners(
                    "requestApproval",
                    data: ["link": Int(link.value), "code": Int(code)])
            case .peerIdentified(let link, let pubkey):
                notifyListeners(
                    "peerIdentified",
                    data: ["link": Int(link.value), "pubkey": pubkey])
            case .confirmIdentityTransfer(let link, let code):
                notifyListeners(
                    "confirmIdentityTransfer",
                    data: ["link": Int(link.value), "code": Int(code)])
            case .identityTransfer(let link, let outcome):
                notifyListeners(
                    "identityTransfer",
                    data: ["link": Int(link.value), "outcome": App.outcomeName(outcome: outcome)])
            case .shareKeyBackup(let path):
                share(URL(fileURLWithPath: path))
            case .wakeAt(let at):
                // Advisory: iOS runs no timer for a suspended app, so this
                // covers the foreground and the next radio callback covers the
                // rest.
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

        try adoptEarlierLayout(in: base, into: directory)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

        var excluded = URLResourceValues()
        excluded.isExcludedFromBackup = true
        try directory.setResourceValues(excluded)

        return directory
    }

    /// Move a store from where earlier builds opened it: a directory named
    /// `dip.sqlite` holding the database, beside the blob directory.
    private func adoptEarlierLayout(in base: URL, into directory: URL) throws {
        let files = FileManager.default
        let earlier = base.appendingPathComponent("dip.sqlite")
        var isDirectory: ObjCBool = false

        guard !files.fileExists(atPath: directory.path),
            files.fileExists(atPath: earlier.path, isDirectory: &isDirectory),
            isDirectory.boolValue
        else { return }

        try files.moveItem(at: earlier, to: directory)

        let blobs = base.appendingPathComponent("blobs")

        if files.fileExists(atPath: blobs.path) {
            try files.moveItem(at: blobs, to: directory.appendingPathComponent("blobs"))
        }
    }

    /// Put the backup in front of the user, and tell the core when it closes.
    ///
    /// The sheet is presented here rather than by the view, which is what keeps
    /// the path on this side of the bridge. `completed` is the sheet's own
    /// answer to whether an activity took the file.
    private func share(_ file: URL) {
        let call = exporting

        exporting = nil

        DispatchQueue.main.async { [weak self] in
            guard let controller = self?.bridge?.viewController else {
                call?.reject("there is nowhere to present the backup")
                return
            }

            let sheet = UIActivityViewController(activityItems: [file], applicationActivities: nil)

            sheet.completionWithItemsHandler = { [weak self] _, completed, _, _ in
                self?.notifyListeners("keyBackupShared", data: ["shared": completed])
                self?.drive { try $0.keyExportFinished() }
                call?.resolve()
            }

            controller.present(sheet, animated: true)
        }
    }

    /// Drop the node and everything driving it, so `open` can run again.
    ///
    /// The store observer and the lifecycle go with their references; the radio
    /// keeps its GATT service, which is registered once at power-on and would
    /// not come back on its own.
    private func close() {
        exporting?.reject("the core closed before the backup was shared")
        exporting = nil
        watching = nil
        lifecycle = nil
        radio.stop()
        core = nil
    }

    /// Write an identity and answer the npub, which is all the view is owed.
    private func adopt(_ secret: Data, into call: CAPPluginCall) {
        do {
            try Keychain.write(secret)
            call.resolve(["npub": try App.identityNpub(secret: secret)])
        } catch {
            call.reject("the identity could not be stored", nil, error)
        }
    }
}

// --------------------------------------------------------------------- Store

/// Tells the view which group of tables moved, so it re-reads what it is
/// showing.
///
/// Held by the core for as long as the subscription lives, so the plugin is a
/// weak reference: the registration outliving the plugin is a leak, not a
/// crash.
private class StoreChanges: StoreObserver {
    private weak var plugin: DipPlugin?

    init(_ plugin: DipPlugin) {
        self.plugin = plugin
    }

    func changed(group: Change) {
        plugin?.notifyListeners("storeChanged", data: ["group": App.changeName(group: group)])
    }
}

// --------------------------------------------------------------------- Radio

extension DipPlugin: RadioDelegate {
    func radio(_ radio: Radio, saw peripheral: String, rssi: Int16) {
        drive { try $0.peripheralSeen(peripheral: PeripheralId(value: peripheral), rssi: rssi) }
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
        notifyListeners("linkClosed", data: ["link": Int(link)])

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

    /// Run one core entry point nobody is waiting on, and carry out what it
    /// answered.
    ///
    /// A `NodeError.Link` is the core refusing to carry on with that link, so
    /// it goes; anything else is logged and the loop continues.
    private func drive(_ call: (Node) throws -> [Action]) {
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
}
