import CoreBluetooth
import Foundation

/// One open L2CAP channel, as the pair of streams CoreBluetooth hands back.
///
/// Everything about framing is the core's: what arrives here is opaque bytes to
/// hand straight back, and what leaves is one length-prefixed fragment the core
/// will not follow until this one is acknowledged. So there is one write in
/// flight and no queue behind it — `docs/transport.md#the-l2cap-bandwidth-upgrade`.
///
/// A channel and the radio holding it are never on two threads at once, because
/// both streams run on the main run loop, which is where CoreBluetooth's own
/// callbacks land with a nil queue.
final class BulkChannel: NSObject, StreamDelegate {
    /// The link this channel belongs to, which is how the radio names it back.
    let link: UInt64

    /// What a channel reports, which is the radio.
    weak var delegate: BulkDelegate?

    /// The channel itself, held for as long as its streams are used: CoreBluetooth
    /// closes a channel nothing references, and the streams then go quiet.
    private let channel: CBL2CAPChannel

    private let input: InputStream
    private let output: OutputStream

    /// The fragment being written, and how much of it has gone.
    private var outgoing: [UInt8] = []
    private var sent = 0

    /// Closing is reported once: both streams fail together and the radio has
    /// only one link to tear down.
    private var closed = false

    init(_ channel: CBL2CAPChannel, on link: UInt64) {
        self.link = link
        self.channel = channel
        self.input = channel.inputStream
        self.output = channel.outputStream
    }

    /// Schedule both streams and start pumping.
    func open() {
        for stream in [input as Stream, output as Stream] {
            stream.delegate = self
            stream.schedule(in: .main, forMode: .default)
            stream.open()
        }
    }

    /// Give up the channel without reporting it, which is what a link going
    /// down for its own reasons wants.
    func close() {
        closed = true

        for stream in [input as Stream, output as Stream] {
            stream.close()
            stream.remove(from: .main, forMode: .default)
        }
    }

    /// Write one fragment, already sized to this channel's MTU by the core.
    ///
    /// A fragment arriving while one is still going is the core's contract
    /// broken, not a queue to grow: it would be reported as written twice.
    func write(_ fragment: Data) {
        guard outgoing.isEmpty else { return fail() }

        outgoing = Array(fragment)
        sent = 0

        drain()
    }

    func stream(_ stream: Stream, handle event: Stream.Event) {
        switch event {
        case .hasBytesAvailable:
            absorb()
        case .hasSpaceAvailable:
            drain()
        case .endEncountered, .errorOccurred:
            fail()
        default:
            break
        }
    }

    /// Push what is left of the fragment, and report it once all of it is gone.
    private func drain() {
        while sent < outgoing.count, output.hasSpaceAvailable {
            let remaining = Array(outgoing[sent...])
            let written = output.write(remaining, maxLength: remaining.count)

            guard written > 0 else { return fail() }

            sent += written
        }

        guard !outgoing.isEmpty, sent == outgoing.count else { return }

        outgoing = []

        // A run loop turn between fragments, because a write that completes
        // here would otherwise release the next one underneath itself and put
        // a whole blob transfer on one stack.
        DispatchQueue.main.async { [weak self] in
            guard let self, !self.closed else { return }

            self.delegate?.bulkWrote(self)
        }
    }

    /// Hand up everything the stream has, in whatever sizes it has it.
    ///
    /// A read is not a fragment: L2CAP is a byte stream, and the core is what
    /// reassembles one off the length prefixes.
    private func absorb() {
        var buffer = [UInt8](repeating: 0, count: Self.readSize)

        while input.hasBytesAvailable {
            let read = input.read(&buffer, maxLength: buffer.count)

            guard read > 0 else { return fail() }

            delegate?.bulkRead(self, bytes: Data(buffer.prefix(read)))
        }
    }

    /// The channel went away, or was never usable. Reported once.
    private func fail() {
        guard !closed else { return }

        close()
        delegate?.bulkClosed(self)
    }

    /// What one read off the input stream takes at most, which is a buffer
    /// rather than a boundary.
    private static let readSize = 8192
}

/// What a bulk channel reports. Every one of these ends at a core entry point.
protocol BulkDelegate: AnyObject {
    func bulkRead(_ channel: BulkChannel, bytes: Data)

    func bulkWrote(_ channel: BulkChannel)

    func bulkClosed(_ channel: BulkChannel)
}
