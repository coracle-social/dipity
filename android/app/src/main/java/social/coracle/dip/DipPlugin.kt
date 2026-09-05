package social.coracle.dip

import com.getcapacitor.JSObject
import com.getcapacitor.Plugin
import com.getcapacitor.PluginCall
import com.getcapacitor.PluginMethod
import com.getcapacitor.annotation.CapacitorPlugin
import uniffi.dip_ffi.coreVersion as loadedCoreVersion

/**
 * The webview's end of the core.
 *
 * One plugin, holding the store and the node, with a method per call the view
 * makes. Everything below the bridge is `dip_ffi`; nothing here decides
 * anything about gossip, the radio or policy.
 *
 * `coreVersion` is the whole surface for now: it proves cargo, uniffi, the
 * jniLibs and this plugin are one chain rather than four things that were built
 * at different times. The radio loop arrives with #50.
 */
@CapacitorPlugin(name = "Dip")
class DipPlugin : Plugin() {
    @PluginMethod
    fun coreVersion(call: PluginCall) {
        call.resolve(JSObject().put("version", loadedCoreVersion()))
    }
}
