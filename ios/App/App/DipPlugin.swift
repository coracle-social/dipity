import Capacitor
import DipFFI
import Foundation
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
        CAPPluginMethod(name: "publish", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "approve", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "exportKey", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "offerIdentity", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "answerIdentityTransfer", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "takeTransferredIdentity", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "listEvents", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "listDetails", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "getEvent", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "wantedBlobs", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "getBlob", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "eventsReferencingBlob", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "preferences", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "preference", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "setPreference", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "clearPreference", returnType: CAPPluginReturnPromise),
    ]

    private let radio = Radio()
    private var lifecycle: Lifecycle?

    /// The `exportKey` call waiting on the sheet it opened.
    private var exporting: CAPPluginCall?

    /// The store and the node, once there is an identity to open them under.
    private var store: Store?
    private var node: Node?

    /// The store registration, live until the plugin drops it.
    private var watching: Subscription?

    // ---------------------------------------------------------------- Identity

    @objc func coreVersion(_ call: CAPPluginCall) {
        call.resolve(["version": DipFFI.coreVersion()])
    }

    @objc func hasIdentity(_ call: CAPPluginCall) {
        call.resolve(["exists": Keychain.has()])
    }

    @objc func createIdentity(_ call: CAPPluginCall) {
        adopt(DipFFI.generateIdentity(), into: call)
    }

    @objc func importIdentity(_ call: CAPPluginCall) {
        guard let nsec = call.getString("nsec") else {
            return call.reject("importIdentity needs an nsec")
        }

        do {
            adopt(try DipFFI.identityFromNsec(nsec: nsec), into: call)
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
        guard node == nil else { return call.resolve() }

        open(into: call)
    }

    private func open(into call: CAPPluginCall) {
        do {
            let directory = try support()
            let opened = try Store.open(
                path: directory.appendingPathComponent("dip.sqlite").path)
            let node = try Node.open(
                store: opened,
                custody: KeychainCustody(),
                directory: directory.path)

            self.store = opened
            self.node = node

            // Registered before the first tick, so nothing the core does on the
            // way up is a change the view never hears about.
            watching = opened.observe(observer: StoreChanges(self))

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

    /// Store and offer an event the view built, with the media it attaches.
    @objc func publish(_ call: CAPPluginCall) {
        guard let node, let event = call.getString("event") else {
            return call.reject("publish needs a started core and an event")
        }

        let media = (call.getArray("media", String.self) ?? []).compactMap {
            Data(base64Encoded: $0)
        }

        do {
            apply(try node.publish(event: event, media: media))
            call.resolve()
        } catch {
            call.reject("that event could not be published", nil, error)
        }
    }

    /// The user answered a `requestApproval` the plugin sent up.
    @objc func approve(_ call: CAPPluginCall) {
        guard let node, let link = call.getInt("link") else {
            return call.reject("approve needs a started core and a link")
        }

        do {
            apply(try node.approve(link: LinkId(value: UInt64(link)), approved: call.getBool("approved", false)))
            call.resolve()
        } catch {
            call.reject("that approval could not be recorded", nil, error)
        }
    }

    /// Write a key backup into the cache directory and ask for the share sheet.
    ///
    /// The path never comes back over the bridge: the view learns only that the
    /// file was shared or dismissed. Answers once the sheet closes, which is
    /// also when the file goes. `docs/keys.md#backup`.
    @objc func exportKey(_ call: CAPPluginCall) {
        guard let node else { return call.reject("exportKey needs a started core") }

        do {
            exporting = call
            apply(
                try node.exportKey(
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
        guard let node, let link = call.getInt("link") else {
            return call.reject("offerIdentity needs a started core and a link")
        }

        do {
            apply(try node.offerIdentity(link: LinkId(value: UInt64(link))))
            call.resolve()
        } catch {
            call.reject("that identity could not be offered", nil, error)
        }
    }

    /// The user answered a `confirmIdentityTransfer` the plugin sent up.
    @objc func answerIdentityTransfer(_ call: CAPPluginCall) {
        guard let node, let link = call.getInt("link") else {
            return call.reject("answerIdentityTransfer needs a started core and a link")
        }

        do {
            apply(
                try node.answerIdentityTransfer(
                    link: LinkId(value: UInt64(link)),
                    confirmed: call.getBool("confirmed", false)))
            call.resolve()
        } catch {
            call.reject("that answer could not be recorded", nil, error)
        }
    }

    /// Adopt the identity an `identityTransfer` of `received` announced.
    ///
    /// The key is handed out once and does not cross the bridge: it is written
    /// to the Keychain here, the same custody path a generated one takes, and
    /// the node is reopened under it. Answers what `start` answers, so the view
    /// reads the new identity off the same field.
    @objc func takeTransferredIdentity(_ call: CAPPluginCall) {
        guard let node, let link = call.getInt("link") else {
            return call.reject("takeTransferredIdentity needs a started core and a link")
        }

        do {
            guard let secret = try node.takeTransferredIdentity(link: LinkId(value: UInt64(link)))
            else {
                return call.reject("no identity arrived on that link")
            }

            try Keychain.write(secret)
            close()
            open(into: call)
        } catch {
            call.reject("the transferred identity could not be adopted", nil, error)
        }
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
        guard let store, let node, let key = call.getString("key"),
            let value = call.getString("value")
        else {
            return call.reject("setPreference needs a started core, a key and a value")
        }

        do {
            try store.setPreference(key: key, value: value)
            apply(try node.policyChanged())
            call.resolve()
        } catch {
            call.reject("that preference could not be written", nil, error)
        }
    }

    @objc func clearPreference(_ call: CAPPluginCall) {
        guard let store, let node, let key = call.getString("key") else {
            return call.reject("clearPreference needs a started core and a key")
        }

        do {
            let existed = try store.clearPreference(key: key)

            apply(try node.policyChanged())
            call.resolve(["existed": existed])
        } catch {
            call.reject("that preference could not be cleared", nil, error)
        }
    }

    /// Run one store read and answer what it gave back under `key`.
    private func answer(_ call: CAPPluginCall, _ key: String, _ read: (Store) throws -> Any) {
        guard let store else { return call.reject("that call needs a started core") }

        do {
            call.resolve([key: try read(store)])
        } catch {
            call.reject("the store could not answer", nil, error)
        }
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
            case .requestApproval(let link):
                notifyListeners("requestApproval", data: ["link": Int(link.value)])
            case .confirmIdentityTransfer(let link, let code):
                notifyListeners(
                    "confirmIdentityTransfer",
                    data: ["link": Int(link.value), "code": Int(code)])
            case .identityTransfer(let link, let outcome):
                notifyListeners(
                    "identityTransfer",
                    data: ["link": Int(link.value), "received": outcome == .received])
            case .shareKeyBackup(let path):
                share(URL(fileURLWithPath: path))
            case .wakeAt(let at):
                // Advisory: iOS runs no timer for a suspended app, so this
                // covers the foreground and the next radio callback covers the
                // rest.
                lifecycle?.wake(at: at)
            case .sendBulk, .publishL2cap, .openL2cap:
                // The bandwidth upgrade is bandwidth, and a link that never
                // gets one still syncs. L2CAP lands with its own change.
                break
            }
        }
    }

    /// Where the store and the blobs live: Application Support, which is backed
    /// up and not purged, unlike Caches.
    private func support() throws -> URL {
        let directory = try FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true)

        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

        return directory
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
        watching = nil
        lifecycle = nil
        radio.stop()
        node = nil
        store = nil
    }

    /// Write an identity and answer the npub, which is all the view is owed.
    private func adopt(_ secret: Data, into call: CAPPluginCall) {
        do {
            try Keychain.write(secret)
            call.resolve(["npub": try DipFFI.identityNpub(secret: secret)])
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
        plugin?.notifyListeners("storeChanged", data: ["group": group.name])
    }
}

extension Change {
    /// The name the view knows this group by.
    fileprivate var name: String {
        switch self {
        case .events: return "events"
        case .blobs: return "blobs"
        case .preferences: return "preferences"
        }
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

    func radio(_ radio: Radio, downOn link: UInt64) {
        drive { try $0.linkDown(link: LinkId(value: link)) }
    }

    func radio(_ radio: Radio, received bytes: Data, on link: UInt64) {
        drive { try $0.bytesReceived(link: LinkId(value: link), write: bytes) }
    }

    func radio(_ radio: Radio, wroteOn link: UInt64) {
        drive { try $0.writeComplete(link: LinkId(value: link)) }
    }

    /// Run one core entry point and carry out what it answered.
    ///
    /// A `NodeError.Link` is the core refusing to carry on with that link, so
    /// it goes; anything else is logged and the loop continues.
    private func drive(_ call: (Node) throws -> [Action]) {
        guard let node else { return }

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
