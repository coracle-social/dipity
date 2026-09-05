import Capacitor
import DipFFI
import Foundation

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
    ]

    private let radio = Radio()

    /// The store and the node, once there is an identity to open them under.
    private var store: Store?
    private var node: Node?

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

            radio.delegate = self

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
                notifyListeners("shareKeyBackup", data: ["path": path])
            case .wakeAt(let at):
                // Advisory, and iOS runs no timer for a suspended app: the next
                // radio callback is the tick. #52 is what makes this more than
                // that while the app is in front.
                notifyListeners("wakeAt", data: ["at": at])
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
    /// A `NodeError.link` is the core refusing to carry on with that link, so
    /// it goes; anything else is logged and the loop continues.
    private func drive(_ call: (Node) throws -> [Action]) {
        guard let node else { return }

        do {
            apply(try call(node))
        } catch let error as NodeError {
            if case .link(let link, _) = error { radio.disconnect(link.value) }

            CAPLog.print("dip: \(error)")
        } catch {
            CAPLog.print("dip: \(error)")
        }
    }
}
