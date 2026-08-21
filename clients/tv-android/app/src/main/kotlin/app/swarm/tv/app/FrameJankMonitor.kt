package app.swarm.tv.app

import android.util.Log
import android.view.Choreographer

/** Debug-only coarse frame monitor; detailed investigation can then use a System Trace around the logged time. */
class FrameJankMonitor(private val choreographer: Choreographer = Choreographer.getInstance()) : Choreographer.FrameCallback {
    private var running = false
    private var previousFrameNanos = 0L

    fun start() {
        if (running) return
        running = true
        previousFrameNanos = 0L
        choreographer.postFrameCallback(this)
    }

    override fun doFrame(frameTimeNanos: Long) {
        if (!running) return
        if (previousFrameNanos != 0L) {
            val frameMillis = (frameTimeNanos - previousFrameNanos) / 1_000_000L
            if (frameMillis >= 34L) Log.w("SwarmFrames", "slow frame: ${frameMillis}ms")
        }
        previousFrameNanos = frameTimeNanos
        choreographer.postFrameCallback(this)
    }

    fun stop() {
        running = false
        choreographer.removeFrameCallback(this)
    }
}
