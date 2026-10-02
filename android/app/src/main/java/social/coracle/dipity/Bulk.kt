package social.coracle.dipity

import android.bluetooth.BluetoothSocket
import java.io.IOException
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

/**
 * One open L2CAP channel, as the socket Android hands back.
 *
 * Everything about framing is the core's: what arrives here is opaque bytes to
 * hand straight back, and what leaves is one length-prefixed fragment the core
 * will not follow until this one is acknowledged. So there is one write in
 * flight and no queue behind it —
 * `docs/transport.md#the-l2cap-bandwidth-upgrade`.
 *
 * A socket's reads and writes both block, so each gets a thread of its own and
 * every report leaves on one — the radio is what puts it back on the radio's own
 * thread. Reporting a write from the writer rather than from the caller is also
 * what keeps a whole blob transfer off one stack.
 */
class BulkChannel(
    val link: ULong,
    private val socket: BluetoothSocket,
    private val delegate: Delegate,
) {
    /** What a channel reports. Every one of these ends at a core entry point. */
    interface Delegate {
        fun bulkRead(channel: BulkChannel, bytes: ByteArray)

        fun bulkWrote(channel: BulkChannel)

        fun bulkClosed(channel: BulkChannel)
    }

    /** Closing happens once, from whichever of the two threads got there. */
    private val closed = AtomicBoolean(false)

    private val writer = Executors.newSingleThreadExecutor()

    /** Start reading. */
    fun open() {
        thread(name = "dip-bulk-$link") { absorb() }
    }

    /**
     * Give up the channel without reporting it, which is what a link going down
     * for its own reasons wants.
     */
    fun close() {
        if (closed.compareAndSet(false, true)) release()
    }

    /** Write one fragment, already sized and length-prefixed by the core. */
    fun write(fragment: ByteArray) {
        try {
            writer.execute { push(fragment) }
        } catch (error: RejectedExecutionException) {
            fail()
        }
    }

    private fun push(fragment: ByteArray) {
        try {
            socket.outputStream.write(fragment)
            socket.outputStream.flush()
        } catch (error: IOException) {
            return fail()
        }

        delegate.bulkWrote(this)
    }

    /**
     * Hand up everything the socket gives, in whatever sizes it gives it.
     *
     * A read is not a fragment: L2CAP is a byte stream, and the core is what
     * reassembles one off the length prefixes.
     */
    private fun absorb() {
        val buffer = ByteArray(READ_SIZE)

        while (!closed.get()) {
            val read =
                try {
                    socket.inputStream.read(buffer)
                } catch (error: IOException) {
                    -1
                }

            if (read <= 0) return fail()

            delegate.bulkRead(this, buffer.copyOf(read))
        }
    }

    /** The channel went away, or was never usable. Reported once. */
    private fun fail() {
        if (closed.compareAndSet(false, true)) {
            release()
            delegate.bulkClosed(this)
        }
    }

    private fun release() {
        writer.shutdownNow()

        try {
            socket.close()
        } catch (error: IOException) {
            // Closing a socket that is already gone is the outcome asked for.
        }
    }

    private companion object {
        /** What one read takes at most, which is a buffer rather than a boundary. */
        const val READ_SIZE = 8192
    }
}
