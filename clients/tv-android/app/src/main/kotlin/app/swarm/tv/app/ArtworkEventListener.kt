package app.swarm.tv.app

import android.util.Log
import coil.EventListener
import coil.decode.DataSource
import coil.request.ImageRequest
import coil.request.SuccessResult

/** Debug-build timing for determining whether a slow frame aligns with artwork I/O or decode. */
class ArtworkEventListener(private val isDebuggable: Boolean) : EventListener {
    private val startedAt = System.nanoTime()
    private var fetchStartedAt = 0L
    private var decodeStartedAt = 0L
    private var fetchMillis = 0L
    private var decodeMillis = 0L

    override fun fetchStart(request: ImageRequest, fetcher: coil.fetch.Fetcher, options: coil.request.Options) {
        fetchStartedAt = System.nanoTime()
    }

    override fun fetchEnd(
        request: ImageRequest,
        fetcher: coil.fetch.Fetcher,
        options: coil.request.Options,
        result: coil.fetch.FetchResult?,
    ) {
        if (fetchStartedAt != 0L) fetchMillis = elapsedMillis(fetchStartedAt)
    }

    override fun decodeStart(request: ImageRequest, decoder: coil.decode.Decoder, options: coil.request.Options) {
        decodeStartedAt = System.nanoTime()
    }

    override fun decodeEnd(
        request: ImageRequest,
        decoder: coil.decode.Decoder,
        options: coil.request.Options,
        result: coil.decode.DecodeResult?,
    ) {
        if (decodeStartedAt != 0L) decodeMillis = elapsedMillis(decodeStartedAt)
    }

    override fun onSuccess(request: ImageRequest, result: SuccessResult) {
        if (!isDebuggable) return
        val totalMillis = elapsedMillis(startedAt)
        if (totalMillis >= 50L || result.dataSource == DataSource.NETWORK) {
            Log.d(
                "SwarmArtwork",
                "source=${result.dataSource} total=${totalMillis}ms fetch=${fetchMillis}ms decode=${decodeMillis}ms url=${request.data}",
            )
        }
    }

    private fun elapsedMillis(startedAt: Long): Long = (System.nanoTime() - startedAt) / 1_000_000L
}
