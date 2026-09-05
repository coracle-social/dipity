package social.coracle.dip

import com.getcapacitor.JSObject
import com.getcapacitor.Plugin
import com.getcapacitor.PluginCall
import com.getcapacitor.PluginMethod
import com.getcapacitor.annotation.CapacitorPlugin
import uniffi.dip_ffi.coreVersion as loadedCoreVersion
import uniffi.dip_ffi.generateIdentity
import uniffi.dip_ffi.identityFromNsec
import uniffi.dip_ffi.identityNpub

/**
 * The webview's end of the core.
 *
 * One plugin, holding the store and the node, with a method per call the view
 * makes. Everything below the bridge is `dip_ffi`; nothing here decides
 * anything about gossip, the radio or policy.
 *
 * The identity methods are the first-run path: the view asks whether there is
 * one, and creates or imports it. Key bytes never come back — a call answers
 * the npub, which is the half that is safe to show.
 */
@CapacitorPlugin(name = "Dip")
class DipPlugin : Plugin() {
    private val keystore by lazy { Keystore(context) }

    @PluginMethod
    fun coreVersion(call: PluginCall) {
        call.resolve(JSObject().put("version", loadedCoreVersion()))
    }

    @PluginMethod
    fun hasIdentity(call: PluginCall) {
        call.resolve(JSObject().put("exists", keystore.has()))
    }

    @PluginMethod
    fun createIdentity(call: PluginCall) {
        store(generateIdentity(), call)
    }

    @PluginMethod
    fun importIdentity(call: PluginCall) {
        val nsec = call.getString("nsec") ?: return call.reject("importIdentity needs an nsec")

        try {
            store(identityFromNsec(nsec), call)
        } catch (error: Exception) {
            call.reject("that is not an nsec", error)
        }
    }

    @PluginMethod
    fun deleteIdentity(call: PluginCall) {
        call.resolve(JSObject().put("existed", keystore.delete()))
    }

    /** Write an identity and answer the npub, which is all the view is owed. */
    private fun store(secret: ByteArray, call: PluginCall) {
        try {
            keystore.write(secret)
            call.resolve(JSObject().put("npub", identityNpub(secret)))
        } catch (error: Exception) {
            call.reject("the identity could not be stored", error)
        }
    }
}
