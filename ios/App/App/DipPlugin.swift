import Capacitor
import Foundation
import LocalAuthentication
import UIKit

/// The webview's end of the core.
///
/// A method per call the view makes, over the core `Encounters` holds for as
/// long as the process lives. The plugin opens nothing it would have to close
/// and only attaches as the view, because the app delegate creates that core at
/// every launch, including one into the background to deliver a Bluetooth event.
/// Everything below the bridge is `dip_ffi`; nothing here decides anything
/// about gossip, the radio schedule or policy.
///
/// # The three call shapes
///
/// A store read answers what it read, through `answer`. A node call answers
/// only the actions it asked for, through `perform`. A radio event has nobody
/// waiting on it, through `Encounters.drive`. Each takes the opened core, runs one entry
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
        CAPPluginMethod(name: "forgetPairing", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "trash", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "restore", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "trashed", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "emptyTrash", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "notificationPermission", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "requestNotificationPermission", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "wantedBlobs", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "getBlob", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "blobPath", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "eventsReferencingBlob", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "policy", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "preferences", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "preference", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "setPreference", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "clearPreference", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "setCanGoBack", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "bluetooth", returnType: CAPPluginReturnPromise),
    ]

    /// The `exportKey` call waiting on the sheet it opened.
    private var exporting: CAPPluginCall?

    /// The core, whoever opened it.
    private var core: Encounters.Core? { Encounters.shared.core }

    /// Attach as the view of the core the app delegate already holds.
    public override func load() {
        Encounters.shared.attach(self)
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

    /// Forget the identity and everything the store gathered under it, closing
    /// the core that runs as it.
    @objc func deleteIdentity(_ call: CAPPluginCall) {
        onMain {
            do {
                try self.core?.store.wipe()
            } catch {
                return call.reject("the store could not be emptied", nil, error)
            }

            Encounters.shared.close()
            call.resolve(["existed": Keychain.delete()])
        }
    }

    // ------------------------------------------------------------------- Node

    /// Open the store and the node, and start the radio, unless the app
    /// delegate already has.
    ///
    /// The view calls this once it knows there is an identity, which is what
    /// makes first run a screen rather than a failed open.
    @objc func start(_ call: CAPPluginCall) {
        onMain {
            self.open(into: call)
        }
    }

    /// Open the core if nobody has, and answer who it runs as.
    private func open(into call: CAPPluginCall) {
        do {
            call.resolve(["identity": try Encounters.shared.open()])
        } catch {
            call.reject("the core could not be opened", nil, error)
        }
    }

    /// The `imeta` entries an event has to carry for a peer to fetch `media`
    /// and check what it gets, base64 in.
    ///
    /// This is the one node call needing no started core, because describing
    /// bytes stores nothing. The view composes the tag, signs the event, and
    /// hands both to `publish`. `docs/storage.md#blob-store`.
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

        // Whoever this displaces is answered rather than left on a promise that
        // never settles, because one sheet means one call waiting on it.
        exporting?.reject("another key export replaced this one")
        exporting = call

        do {
            Encounters.shared.apply(
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
    /// to the Keychain here, the same custody path a generated one takes, the
    /// store is emptied of the first-run identity's data, and the node is
    /// reopened under it. It answers what `start` answers, with the new identity
    /// on the same field.
    @objc func takeTransferredIdentity(_ call: CAPPluginCall) {
        onMain { self.adoptTransferred(into: call) }
    }

    private func adoptTransferred(into call: CAPPluginCall) {
        guard let core else { return call.reject("takeTransferredIdentity needs a started core") }
        guard let link = link(call) else {
            return call.reject("takeTransferredIdentity needs a link")
        }

        do {
            guard var secret = try core.node.takeTransferredIdentity(link: link) else {
                return call.reject("no identity arrived on that link")
            }
            defer { secret.resetBytes(in: 0..<secret.count) }

            try Keychain.write(secret)

            // What the first-run identity gathered was its own, and goes with it.
            try core.store.wipe()
            Encounters.shared.close()
            open(into: call)

            for group in [Change.events, .blobs, .preferences] {
                notifyListeners("storeChanged", data: ["group": App.changeName(group: group)])
            }
        } catch {
            call.reject("the transferred identity could not be adopted", nil, error)
        }
    }

    // ------------------------------------------------------------- Bluetooth

    /// Whether Bluetooth can be used now. Changes arrive as `bluetooth` events.
    @objc func bluetooth(_ call: CAPPluginCall) {
        onMain { call.resolve(["state": Encounters.shared.power]) }
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

    /// Stop recognizing somebody's device until the two next sync.
    @objc func forgetPairing(_ call: CAPPluginCall) {
        guard let pubkey = call.getString("pubkey") else {
            return call.reject("forgetPairing needs a pubkey")
        }

        answer(call, "existed") { try $0.forgetPairing(pubkey: pubkey) }
    }

    /// Put an event in the trash, which retracts it at once if the user wrote it.
    @objc func trash(_ call: CAPPluginCall) {
        guard let id = call.getString("id") else { return call.reject("trash needs an id") }

        perform(call, "that could not be trashed") { try $0.node.trash(id: id) }
    }

    /// Take an event back out of the trash, which restores it for peers too if the user wrote it.
    @objc func restore(_ call: CAPPluginCall) {
        guard let id = call.getString("id") else { return call.reject("restore needs an id") }

        perform(call, "that could not be restored") { try $0.node.restore(id: id) }
    }

    @objc func trashed(_ call: CAPPluginCall) {
        answer(call, "trashed") { try $0.trashed() }
    }

    /// Whether the user has let the app notify: `granted`, `denied` or `prompt`.
    @objc func notificationPermission(_ call: CAPPluginCall) {
        Alerts.permission { call.resolve(["permission": $0]) }
    }

    /// Ask the user to let the app notify, if they have not been asked.
    @objc func requestNotificationPermission(_ call: CAPPluginCall) {
        Alerts.request { call.resolve(["permission": $0]) }
    }

    /// Delete everything in the trash from this device.
    @objc func emptyTrash(_ call: CAPPluginCall) {
        perform(call, "the trash could not be emptied") { try $0.node.emptyTrash() }
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

    /// The file holding a whole blob, for the view to draw.
    @objc func blobPath(_ call: CAPPluginCall) {
        guard let sha256 = call.getString("sha256") else {
            return call.reject("blobPath needs a sha256")
        }

        answer(call, "path") { try $0.blobPath(sha256: sha256) ?? NSNull() }
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

                Encounters.shared.apply(try core.node.policyChanged())
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
        _ call: CAPPluginCall, _ failure: String, _ body: @escaping (Encounters.Core) throws -> [Action]
    ) {
        onMain {
            guard let core = self.core else { return call.reject("that call needs a started core") }

            do {
                Encounters.shared.apply(try body(core))
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
    /// A negative link is refused here rather than trapping on the conversion.
    /// A link is a `UInt64` the shell assigned.
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

    /// Write an identity and answer the npub, which is all the view is owed.
    ///
    /// The bytes are wiped on the way out, whichever way it went.
    private func adopt(_ secret: Data, into call: CAPPluginCall) {
        var secret = secret
        defer { secret.resetBytes(in: 0..<secret.count) }

        do {
            try Keychain.write(secret)
            call.resolve(["npub": try App.identityNpub(secret: secret)])
        } catch {
            call.reject("the identity could not be stored", nil, error)
        }
    }
}

// ---------------------------------------------------------------------- View

extension DipPlugin: EncountersView {
    func notify(_ event: String, _ data: [String: Any]) {
        notifyListeners(event, data: data)
    }

    /// Put the backup in front of the user, and tell the core when it closes.
    ///
    /// The sheet is presented here rather than by the view, which is what keeps
    /// the path on this side of the bridge. `completed` is the sheet's own
    /// answer to whether an activity took the file.
    func share(_ file: URL) -> Bool {
        guard let call = exporting else { return false }

        exporting = nil

        guard let controller = bridge?.viewController else {
            call.reject("there is nowhere to present the backup")
            return false
        }

        let sheet = UIActivityViewController(activityItems: [file], applicationActivities: nil)

        sheet.completionWithItemsHandler = { [weak self] _, completed, _, _ in
            self?.notifyListeners("keyBackupShared", data: ["shared": completed])
            Encounters.shared.drive { try $0.keyExportFinished() }
            call.resolve()
        }

        // An iPad shows the sheet as a popover, and UIKit raises if a popover has nothing to point at.
        if let popover = sheet.popoverPresentationController {
            let bounds = controller.view.bounds

            popover.sourceView = controller.view
            popover.sourceRect = CGRect(x: bounds.midX, y: bounds.midY, width: 0, height: 0)
            popover.permittedArrowDirections = []
        }

        controller.present(sheet, animated: true)

        return true
    }
}
