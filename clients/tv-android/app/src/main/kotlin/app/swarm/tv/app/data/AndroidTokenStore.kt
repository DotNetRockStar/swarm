/**
 * Encrypted-at-rest STUN access token storage for Android — implements
 * `:core`'s [TokenStore] using `androidx.security.crypto`'s
 * `EncryptedSharedPreferences`, which wraps a Keystore-backed AES256-GCM
 * master key so the token never touches disk in the clear (matches the
 * plan's "access token stored encrypted" requirement). The Rust side of
 * SWARM uses the platform keychain via the `keyring` crate; this is the
 * Android-native equivalent of the same idea.
 */
package app.swarm.tv.app.data

import android.content.Context
import android.content.SharedPreferences
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import app.swarm.tv.core.token.TokenStore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

private const val PREFS_NAME = "swarm_secure_prefs"
private const val KEY_ACCESS_TOKEN = "access_token"

class AndroidTokenStore(context: Context) : TokenStore {
    private val prefs: SharedPreferences by lazy {
        retryTransientKeystoreFailure {
            val masterKey = MasterKey.Builder(context.applicationContext)
                .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
                .build()
            EncryptedSharedPreferences.create(
                context.applicationContext,
                PREFS_NAME,
                masterKey,
                EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
                EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
            )
        }
    }

    override suspend fun save(token: String) = withContext(Dispatchers.IO) {
        prefs.edit().putString(KEY_ACCESS_TOKEN, token).apply()
    }

    override suspend fun load(): String? = withContext(Dispatchers.IO) {
        prefs.getString(KEY_ACCESS_TOKEN, null)
    }

    override suspend fun clear() = withContext(Dispatchers.IO) {
        prefs.edit().remove(KEY_ACCESS_TOKEN).apply()
    }
}
