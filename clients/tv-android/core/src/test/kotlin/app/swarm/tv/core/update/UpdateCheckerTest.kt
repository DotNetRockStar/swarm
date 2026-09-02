package app.swarm.tv.core.update

import kotlinx.coroutines.test.runTest
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.jupiter.api.AfterEach
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertInstanceOf
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.BeforeEach
import org.junit.jupiter.api.Test
import java.io.File
import java.nio.file.Files
import java.security.MessageDigest

class UpdateCheckerTest {
    private lateinit var server: MockWebServer

    @BeforeEach fun setUp() { server = MockWebServer(); server.start() }
    @AfterEach fun tearDown() { server.shutdown() }

    private fun checker() = UpdateChecker(manifestUrl = server.url("/tv-latest.json").toString())

    private fun manifestJson(versionCode: Int, apkUrl: String, sha256: String) = """
        {"version_code":$versionCode,"version_name":"0.1.$versionCode","notes":"stuff",
         "min_sdk_version":25,
         "assets":{"arm64-v8a":{"url":"$apkUrl","sha256":"$sha256"}}}
    """.trimIndent()

    @Test
    fun `reports up to date when the manifest is not newer`() = runTest {
        server.enqueue(MockResponse().setBody(manifestJson(5, "http://x/app.apk", "00")))
        val status = checker().check(currentVersionCode = 5, supportedAbis = listOf("arm64-v8a"))
        assertEquals(UpdateStatus.UpToDate, status)
    }

    @Test
    fun `reports an available update with the matching ABI asset`() = runTest {
        server.enqueue(MockResponse().setBody(manifestJson(9, "http://x/app.apk", "abc")))
        val status = checker().check(currentVersionCode = 5, supportedAbis = listOf("arm64-v8a", "armeabi-v7a"))
        val available = assertInstanceOf(UpdateStatus.Available::class.java, status)
        assertEquals(9L, available.manifest.versionCode)
        assertEquals("http://x/app.apk", available.asset.url)
    }

    @Test
    fun `errors when no asset matches the device ABI`() = runTest {
        server.enqueue(MockResponse().setBody(manifestJson(9, "http://x/app.apk", "abc")))
        val status = checker().check(currentVersionCode = 5, supportedAbis = listOf("x86"))
        assertInstanceOf(UpdateStatus.Error::class.java, status)
    }

    @Test
    fun `download verifies the checksum`() = runTest {
        val payload = "fake apk bytes"
        val sha = MessageDigest.getInstance("SHA-256")
            .digest(payload.toByteArray()).joinToString("") { "%02x".format(it) }
        server.enqueue(MockResponse().setBody(payload))
        val target = File(Files.createTempDirectory("upd").toFile(), "app.apk")
        val out = checker().download(UpdateAsset(server.url("/app.apk").toString(), sha), target)
        assertTrue(out.exists())
        assertEquals(payload.toByteArray().size.toLong(), out.length())
    }

    @Test
    fun `download rejects a bad checksum`() = runTest {
        server.enqueue(MockResponse().setBody("bytes"))
        val target = File(Files.createTempDirectory("upd").toFile(), "app.apk")
        try {
            checker().download(UpdateAsset(server.url("/app.apk").toString(), "deadbeef"), target)
            throw AssertionError("expected a checksum failure")
        } catch (expected: Exception) {
            assertTrue(!target.exists(), "a corrupt download must not be left on disk")
        }
    }
}
