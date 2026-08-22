/**
 * Parental content controls — Room-backed via
 * [app.swarm.tv.app.data.db.KidModeSettingsEntity] (singleton row, same
 * pattern as [AndroidAppSettingsStore]). The PIN gates *managing* this
 * feature (opening this section in Settings to view/edit/turn it off), not
 * a session-level unlock exposed anywhere else in the app — once enabled,
 * content restriction just applies everywhere content is browsable until
 * someone re-enters the PIN here to change it.
 */
package app.swarm.tv.app.data

import android.content.Context
import app.swarm.tv.app.data.db.AppDatabase
import app.swarm.tv.app.data.db.KidModeSettingsEntity
import app.swarm.tv.core.peer.MediaKind
import java.security.MessageDigest
import java.security.SecureRandom
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.withContext

data class KidModeSettings(
    val enabled: Boolean,
    val pinHash: String,
    val pinSalt: String,
    val allowedKinds: Set<MediaKind>,
    /** Null means every genre is allowed. */
    val allowedGenres: Set<String>?,
    val maxMovieRating: String?,
    val maxTvRating: String?,
) {
    fun pinMatches(pin: String): Boolean = hashPin(pin, pinSalt) == pinHash
}

/**
 * A small, curated maturity ordering for exactly the two US rating scales
 * this app's scraper writes (see `swarm-media`'s TMDb certification
 * scraping) — movies and TV shows use different, incompatible scales, so
 * each gets its own ordered list rather than trying to invent a unified
 * cross-scale ranking nobody asked for. Music carries no rating concept at
 * all (see [app.swarm.tv.core.peer.CatalogEntry.rating]'s own doc comment),
 * so Kid Mode's music restriction is kind-only, never rating-based.
 */
object RatingScale {
    val MOVIE_ORDER = listOf("G", "PG", "PG-13", "R", "NC-17")
    val TV_ORDER = listOf("TV-Y", "TV-Y7", "TV-G", "TV-PG", "TV-14", "TV-MA")

    /**
     * A `null` [max] means no restriction is configured. Once a parent sets
     * a maximum, missing or unfamiliar ratings must fail closed: showing an
     * unrated R movie under a PG-13 limit is a much worse surprise than
     * temporarily hiding a title until metadata scraping identifies it.
     */
    fun isAllowed(rating: String?, max: String?, order: List<String>): Boolean {
        if (max == null) return true
        if (rating == null) return false
        val ratingIndex = order.indexOf(rating)
        val maxIndex = order.indexOf(max)
        if (ratingIndex == -1 || maxIndex == -1) return false
        return ratingIndex <= maxIndex
    }
}

private fun hashPin(pin: String, salt: String): String {
    val digest = MessageDigest.getInstance("SHA-256").digest((salt + pin).toByteArray(Charsets.UTF_8))
    return digest.joinToString("") { "%02x".format(it) }
}

private fun randomSalt(): String {
    val bytes = ByteArray(16)
    SecureRandom().nextBytes(bytes)
    return bytes.joinToString("") { "%02x".format(it) }
}

class AndroidKidModeStore(context: Context) {
    private val dao = AppDatabase.getInstance(context).kidModeDao()

    suspend fun get(): KidModeSettings? = withContext(Dispatchers.IO) { dao.get()?.toDomain() }

    fun observe() = dao.observe().map { it?.toDomain() }

    /** Turns Kid Mode on (or fully replaces an existing PIN + rule set) — [pin] is hashed with a freshly generated salt, never stored as-is. */
    suspend fun enable(
        pin: String,
        allowedKinds: Set<MediaKind>,
        allowedGenres: Set<String>?,
        maxMovieRating: String?,
        maxTvRating: String?,
    ) = withContext(Dispatchers.IO) {
        val salt = randomSalt()
        dao.upsert(
            KidModeSettingsEntity(
                enabled = true,
                pinHash = hashPin(pin, salt),
                pinSalt = salt,
                allowedKinds = allowedKinds.joinToString(",") { it.name },
                allowedGenres = allowedGenres?.joinToString(","),
                maxMovieRating = maxMovieRating,
                maxTvRating = maxTvRating,
            ),
        )
    }

    /** Edits the content rules on an already-enabled Kid Mode, keeping the existing PIN untouched. No-op if Kid Mode isn't enabled yet. */
    suspend fun updateRules(
        allowedKinds: Set<MediaKind>,
        allowedGenres: Set<String>?,
        maxMovieRating: String?,
        maxTvRating: String?,
    ) = withContext(Dispatchers.IO) {
        val current = dao.get() ?: return@withContext
        dao.upsert(
            current.copy(
                allowedKinds = allowedKinds.joinToString(",") { it.name },
                allowedGenres = allowedGenres?.joinToString(","),
                maxMovieRating = maxMovieRating,
                maxTvRating = maxTvRating,
            ),
        )
    }

    suspend fun disable() = withContext(Dispatchers.IO) { dao.clear() }
}

private fun KidModeSettingsEntity.toDomain() = KidModeSettings(
    enabled = enabled,
    pinHash = pinHash,
    pinSalt = pinSalt,
    allowedKinds = allowedKinds.split(",").filter { it.isNotEmpty() }.mapNotNull { runCatching { MediaKind.valueOf(it) }.getOrNull() }.toSet(),
    allowedGenres = allowedGenres?.split(",")?.filter { it.isNotEmpty() }?.toSet(),
    maxMovieRating = maxMovieRating,
    maxTvRating = maxTvRating,
)
