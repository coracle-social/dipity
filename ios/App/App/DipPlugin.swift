import Capacitor
import DipFFI
import Foundation

/// The webview's end of the core.
///
/// One plugin, holding the store and the node, with a method per call the view
/// makes. Everything below the bridge is `dip_ffi`; nothing here decides
/// anything about gossip, the radio or policy.
///
/// The identity methods are the first-run path: the view asks whether there is
/// one, and creates or imports it. Key bytes never come back — a call answers
/// the npub, which is the half that is safe to show.
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
    ]

    @objc func coreVersion(_ call: CAPPluginCall) {
        call.resolve(["version": DipFFI.coreVersion()])
    }

    @objc func hasIdentity(_ call: CAPPluginCall) {
        call.resolve(["exists": Keychain.has()])
    }

    @objc func createIdentity(_ call: CAPPluginCall) {
        store(DipFFI.generateIdentity(), into: call)
    }

    @objc func importIdentity(_ call: CAPPluginCall) {
        guard let nsec = call.getString("nsec") else {
            return call.reject("importIdentity needs an nsec")
        }

        do {
            store(try DipFFI.identityFromNsec(nsec: nsec), into: call)
        } catch {
            call.reject("that is not an nsec", nil, error)
        }
    }

    @objc func deleteIdentity(_ call: CAPPluginCall) {
        call.resolve(["existed": Keychain.delete()])
    }

    /// Write an identity and answer the npub, which is all the view is owed.
    private func store(_ secret: Data, into call: CAPPluginCall) {
        do {
            try Keychain.write(secret)
            call.resolve(["npub": try DipFFI.identityNpub(secret: secret)])
        } catch {
            call.reject("the identity could not be stored", nil, error)
        }
    }
}
