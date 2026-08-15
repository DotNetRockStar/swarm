/**
 * Device identity certificate, Android-native equivalent of
 * `swarm-p2p::identity::ensure_identity` (Rust): a long-lived self-signed
 * certificate whose SHA-256 fingerprint is the device's peer identity,
 * submitted to the STUN server at registration and later pinned by peers.
 *
 * Unlike the Rust side (which writes cert/key PEM files and can hand the
 * private key directly to a TLS library), the private key here lives in
 * `AndroidKeyStore` and is deliberately **non-exportable** — Android
 * generates the self-signed certificate as part of key generation, so no
 * key material is ever readable outside secure hardware where available.
 * A `KeyManagerFactory` initialized from this same `KeyStore` alias can
 * still be handed to a TLS/QUIC stack for mutual auth (standard Java TLS
 * pattern), which is what wires this into the peer QUIC connection once
 * that transport lands (see the loopback-proxy note in `docs/PROTOCOL.md`).
 */
package app.swarm.tv.app.data

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.math.BigInteger
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.MessageDigest
import java.security.PrivateKey
import java.security.cert.X509Certificate
import java.util.Date
import javax.security.auth.x500.X500Principal

private const val KEYSTORE_PROVIDER = "AndroidKeyStore"
private const val KEY_ALIAS = "swarm_device_identity"
private val VALIDITY_YEARS = 20L

object AndroidDeviceIdentity {
    /** Loads (or generates, on first run) the device identity and returns its fingerprint. */
    fun ensureFingerprint(): String {
        val digest = MessageDigest.getInstance("SHA-256").digest(certificate().encoded)
        return digest.joinToString(separator = "") { "%02x".format(it) }
    }

    /** The device's self-signed certificate — safe to hand to a peer during the mTLS handshake. */
    fun certificate(): X509Certificate = keyStore().getCertificate(KEY_ALIAS) as X509Certificate

    /**
     * A non-exportable `AndroidKeyStore`-backed private key handle: usable by any Java crypto API
     * that signs through the provider (what a `KeyManagerFactory`/TLS stack does), but
     * `getEncoded()` on it always returns null — the raw key material never leaves the Keystore.
     */
    fun privateKey(): PrivateKey = keyStore().getKey(KEY_ALIAS, null) as PrivateKey

    private fun keyStore(): KeyStore {
        val keyStore = KeyStore.getInstance(KEYSTORE_PROVIDER).apply { load(null) }
        if (!keyStore.containsAlias(KEY_ALIAS)) {
            generateKeyPair()
        }
        return keyStore
    }

    private fun generateKeyPair() {
        val notBefore = Date()
        val notAfter = Date(notBefore.time + VALIDITY_YEARS * 365 * 24 * 60 * 60 * 1000)
        val spec = KeyGenParameterSpec.Builder(
            KEY_ALIAS,
            KeyProperties.PURPOSE_SIGN or KeyProperties.PURPOSE_VERIFY,
        )
            .setDigests(KeyProperties.DIGEST_SHA256)
            .setCertificateSubject(X500Principal("CN=swarm-device"))
            .setCertificateSerialNumber(BigInteger.ONE)
            .setKeyValidityStart(notBefore)
            .setKeyValidityEnd(notAfter)
            .build()
        val generator = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, KEYSTORE_PROVIDER)
        generator.initialize(spec)
        generator.generateKeyPair()
    }
}
