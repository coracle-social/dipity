import Foundation
import UIKit

/// Where the app is, what the battery is at, and when the core wants ticking.
///
/// Three things the core asks for and nothing answered. Each is small on its
/// own, and each is a subsystem that silently does nothing without it:
/// `gate.presence` stays `None` until the shell first reports it, blob
/// transfers are metered against a battery level that was never reported, and a
/// `WakeAt` nobody acts on leaves a session holding one of six link slots until
/// the radio happens to fire.
///
/// The `WakeAt` timer is honest about what it can do. iOS runs no timer for a
/// suspended app, so this covers the foreground and the next radio callback
/// covers the rest — which is why the action is advisory rather than a promise.
final class Lifecycle {
    /// What to do when one of the three moves.
    private let foregrounded: () -> Void
    private let backgrounded: () -> Void
    private let battery: (UInt8) -> Void
    private let tick: () -> Void

    /// The next tick the core asked for, if the app is in front to run it.
    private var timer: Timer?

    init(
        foregrounded: @escaping () -> Void,
        backgrounded: @escaping () -> Void,
        battery: @escaping (UInt8) -> Void,
        tick: @escaping () -> Void
    ) {
        self.foregrounded = foregrounded
        self.backgrounded = backgrounded
        self.battery = battery
        self.tick = tick

        UIDevice.current.isBatteryMonitoringEnabled = true

        let notifications = NotificationCenter.default

        notifications.addObserver(
            self, selector: #selector(didBecomeActive),
            name: UIApplication.didBecomeActiveNotification, object: nil)
        notifications.addObserver(
            self, selector: #selector(didEnterBackground),
            name: UIApplication.didEnterBackgroundNotification, object: nil)
        notifications.addObserver(
            self, selector: #selector(batteryChanged),
            name: UIDevice.batteryLevelDidChangeNotification, object: nil)
    }

    deinit {
        timer?.invalidate()
        NotificationCenter.default.removeObserver(self)
    }

    /// Report where the app is and what the battery is at, right now.
    ///
    /// Called once the node exists: the notifications only fire on a change,
    /// and a device that is launched and left alone would otherwise be deciding
    /// on missing information until the first one.
    func report() {
        if UIApplication.shared.applicationState == .background {
            backgrounded()
        } else {
            foregrounded()
        }

        reportBattery()
    }

    /// Tick at or after `at`, if the app is still in front then.
    func wake(at: Int64) {
        let after = TimeInterval(at) - Date().timeIntervalSince1970

        timer?.invalidate()

        guard after > 0 else { return tick() }

        timer = Timer.scheduledTimer(withTimeInterval: after, repeats: false) { [weak self] _ in
            self?.tick()
        }
    }

    @objc private func didBecomeActive() {
        foregrounded()
        reportBattery()
    }

    @objc private func didEnterBackground() {
        // Nothing runs a timer for a suspended app, and leaving one armed means
        // firing late into a state the core has moved on from.
        timer?.invalidate()
        timer = nil

        backgrounded()
    }

    @objc private func batteryChanged() {
        reportBattery()
    }

    /// The level as a percentage. `-1` is the simulator, or monitoring that has
    /// not settled yet, and is not worth reporting as a flat battery.
    private func reportBattery() {
        let level = UIDevice.current.batteryLevel

        guard level >= 0 else { return }

        battery(UInt8((level * 100).rounded()))
    }
}
