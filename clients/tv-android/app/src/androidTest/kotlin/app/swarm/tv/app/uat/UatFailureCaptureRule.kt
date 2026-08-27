package app.swarm.tv.app.uat

import android.util.Log
import androidx.compose.ui.test.junit4.ComposeTestRule
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.printToString
import androidx.test.core.app.ApplicationProvider
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.UiDevice
import java.io.File
import org.junit.rules.TestWatcher
import org.junit.runner.Description

/**
 * On any test failure, dumps everything useful for debugging directly on
 * the device, under [failureDir], for `scripts/tv_uat_suite.sh` to `adb
 * pull` afterward: a screenshot, the UIAutomator window-hierarchy XML, the
 * Compose semantics tree, and a single structured logcat line the
 * orchestration script greps for as a checkpoint to also pull the matching
 * server-side SQLite/log evidence. See the plan's "full dump ... from UI to
 * media server" requirement and the swarm-tv-uat-suite skill.
 *
 * [composeTestRuleProvider] is a lazy accessor (not a direct constructor
 * param) because [UatTestBase] constructs this rule before its own
 * `composeTestRule` property is guaranteed initialized — both are plain
 * `val`s applied via the same `RuleChain`.
 */
class UatFailureCaptureRule(
    private val composeTestRuleProvider: () -> ComposeTestRule,
) : TestWatcher() {

    override fun failed(e: Throwable, description: Description) {
        val testId = "${description.className}#${description.methodName}"
        val dir = failureDir(description)
        dir.mkdirs()

        runCatching {
            device().takeScreenshot(File(dir, "screenshot.png"))
        }.onFailure { Log.e(TAG, "screenshot capture failed for $testId", it) }

        runCatching {
            device().dumpWindowHierarchy(File(dir, "hierarchy.xml"))
        }.onFailure { Log.e(TAG, "hierarchy dump failed for $testId", it) }

        runCatching {
            val semantics = composeTestRuleProvider().onRoot(useUnmergedTree = true).printToString()
            File(dir, "compose_semantics.txt").writeText(semantics)
        }.onFailure { Log.e(TAG, "compose semantics dump failed for $testId", it) }

        runCatching {
            File(dir, "failure.txt").writeText(
                "test=$testId\nmessage=${e.message}\n\n${Log.getStackTraceString(e)}",
            )
        }

        // The host-side orchestration script watches logcat for this exact
        // line (see FAILURE_MARKER_PATTERN in scripts/tv_uat_suite.sh) as
        // the checkpoint to pull this directory and cross-reference
        // server-side SQLite/log state at the same instant.
        Log.e(
            TAG,
            "UAT_TEST_FAILED test=$testId device_ts=${System.currentTimeMillis()} " +
                "dump_dir=${dir.absolutePath}",
        )
    }

    private fun device(): UiDevice = UiDevice.getInstance(InstrumentationRegistry.getInstrumentation())

    private fun failureDir(description: Description): File {
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        val safeName = "${description.className}_${description.methodName}"
            .replace(Regex("[^A-Za-z0-9._-]"), "_")
        // External files dir: readable via a plain `adb pull` (no `run-as`
        // needed), unlike the app's private data dir the rest of this
        // suite reads via `run-as` for swarm.db/SharedPreferences.
        return File(context.getExternalFilesDir(null), "$UAT_FAILURES_DIR/$safeName")
    }

    companion object {
        private const val TAG = "UAT"

        /** Documented on-device path fragment `tv_uat_suite.sh` pulls via adb — keep both in sync. */
        const val UAT_FAILURES_DIR = "uat-failures"
    }
}
