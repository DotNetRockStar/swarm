/**
 * `AndroidKeyStore`'s daemon can transiently fail the very first key
 * operation of a freshly-updated app's cold start — observed as a
 * reproducible crash on real Fire TV hardware: an in-place APK update
 * (`adb install -r`, matching how this app is actually distributed) always
 * crashed on the very next launch and always succeeded on the one after
 * that, with no other change. The daemon needs a brief window after
 * `ACTION_MY_PACKAGE_REPLACED` to reconcile the app UID's key state, and an
 * `AndroidKeyStore` touch inside that window can throw even though the same
 * call succeeds moments later. [AndroidTokenStore] and [AndroidDeviceIdentity]
 * both hit this on their first Keystore access; retrying the same call a few
 * times a short moment apart reproduces the same self-heal a real second
 * launch gets, without requiring the user to relaunch the app themselves.
 */
package app.swarm.tv.app.data

internal fun <T> retryTransientKeystoreFailure(attempts: Int = 3, delayMillis: Long = 200, block: () -> T): T {
    repeat(attempts) { attempt ->
        try {
            return block()
        } catch (error: Exception) {
            if (attempt == attempts - 1) throw error
            Thread.sleep(delayMillis)
        }
    }
    error("unreachable")
}
