/**
 * Self-signed X.509 identity generator for tests only. Production Android
 * code uses `AndroidKeyStore` instead (`:app`'s `AndroidDeviceIdentity`) —
 * this exists purely so JVM tests (no Android framework available) can
 * exercise [app.swarm.tv.core.transport.PeerQuicClient] against a real
 * server, using Bouncy Castle to build the certificate.
 */
package app.swarm.tv.core.transport

import java.math.BigInteger
import java.security.KeyPairGenerator
import java.security.MessageDigest
import java.security.PrivateKey
import java.security.cert.X509Certificate
import java.security.spec.ECGenParameterSpec
import java.time.Duration
import java.time.Instant
import java.util.Date
import org.bouncycastle.asn1.x500.X500Name
import org.bouncycastle.cert.jcajce.JcaX509CertificateConverter
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder
import org.bouncycastle.jce.provider.BouncyCastleProvider
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder

data class TestIdentity(val certificate: X509Certificate, val privateKey: PrivateKey) {
    val fingerprint: String by lazy {
        MessageDigest.getInstance("SHA-256").digest(certificate.encoded).joinToString("") { "%02x".format(it) }
    }

    companion object {
        private val bouncyCastle = BouncyCastleProvider()

        fun generate(commonName: String = "swarm-test-identity"): TestIdentity {
            val keyPair = KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }.generateKeyPair()
            val now = Instant.now()
            val subject = X500Name("CN=$commonName")
            val builder = JcaX509v3CertificateBuilder(
                subject,
                BigInteger.valueOf(now.toEpochMilli()),
                Date.from(now.minus(Duration.ofMinutes(5))),
                Date.from(now.plus(Duration.ofDays(3650))),
                subject,
                keyPair.public,
            )
            val signer = JcaContentSignerBuilder("SHA256withECDSA").setProvider(bouncyCastle).build(keyPair.private)
            val certificate = JcaX509CertificateConverter().setProvider(bouncyCastle).getCertificate(builder.build(signer))
            return TestIdentity(certificate, keyPair.private)
        }
    }
}
