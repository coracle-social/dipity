import Capacitor
import DipFFI
import Foundation

/// The webview's end of the core.
///
/// One plugin, holding the store and the node, with a method per call the view
/// makes. Everything below the bridge is `dip_ffi`; nothing here decides
/// anything about gossip, the radio or policy.
///
/// `coreVersion` is the whole surface for now: it proves cargo, uniffi, the
/// xcframework and this plugin are one chain rather than four things that were
/// built at different times. The radio loop arrives with #49.
@objc(DipPlugin)
public class DipPlugin: CAPPlugin, CAPBridgedPlugin {
    public let identifier = "DipPlugin"
    public let jsName = "Dip"
    public let pluginMethods: [CAPPluginMethod] = [
        CAPPluginMethod(name: "coreVersion", returnType: CAPPluginReturnPromise)
    ]

    @objc func coreVersion(_ call: CAPPluginCall) {
        call.resolve(["version": DipFFI.coreVersion()])
    }
}
