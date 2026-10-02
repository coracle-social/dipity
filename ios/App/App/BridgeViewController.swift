import Capacitor
import UIKit

/// The webview's controller, which registers the plugin Capacitor cannot find
/// on its own: only plugins installed from npm are listed in the generated
/// config, and `DipPlugin` lives in the App target.
class BridgeViewController: CAPBridgeViewController {
    override open func capacitorDidLoad() {
        bridge?.registerPluginInstance(DipPlugin())
    }
}
