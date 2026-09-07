import Foundation
import os

/// The core's `log` records, on the unified log.
///
/// The Rust module path arrives as the record's target and becomes the
/// category, so `dip::sync::blob` filters on its own in Console without the
/// core naming a subsystem it cannot see. Records come in on whichever thread
/// logged them, which `os.Logger` takes from any.
///
/// `os.Logger` and the core's `Logger` are two protocols of that name in this
/// module, so both are spelled out.
final class OsLog: App.Logger {
    private static let subsystem = Bundle.main.bundleIdentifier ?? "social.coracle.dip"

    /// Route the core's log here, at the level this build carries.
    ///
    /// Called once, before the core is opened, so a failure on the way up is
    /// logged rather than being the first thing nobody sees.
    static func install() {
        #if DEBUG
            let level = LogLevel.debug
        #else
            let level = LogLevel.info
        #endif

        _ = App.initLogging(logger: OsLog(), level: level)
    }

    func log(level: LogLevel, target: String, message: String) {
        // A dynamic string is redacted to <private> unless it says otherwise,
        // and a log the developer cannot read is what this exists to end.
        os.Logger(subsystem: Self.subsystem, category: target)
            .log(level: Self.severity(level), "\(message, privacy: .public)")
    }

    /// The unified log has no warning, so a warning is `default` — one step
    /// above info, which keeps the order the core logged in.
    private static func severity(_ level: LogLevel) -> OSLogType {
        switch level {
        case .error:
            return .error
        case .warn:
            return .default
        case .info:
            return .info
        case .debug:
            return .debug
        case .trace:
            return .debug
        }
    }
}
