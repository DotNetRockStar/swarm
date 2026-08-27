package app.swarm.tv.app.uat

import android.content.Intent
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.rules.ActivityScenarioRule
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
import app.swarm.tv.app.MainActivity
import app.swarm.tv.app.data.AndroidLikedEntriesStore
import app.swarm.tv.app.data.AndroidWatchStateStore
import app.swarm.tv.app.data.AndroidWatchlistStore
import app.swarm.tv.app.data.db.AppDatabase
import app.swarm.tv.app.ui.UatTestTags
import org.junit.Before
import org.junit.Rule
import org.junit.rules.RuleChain
import org.junit.rules.TestRule

/**
 * Shared setup for every scenario in the UAT suite (see the
 * swarm-tv-uat-suite skill). Launches [MainActivity] with the same
 * debug-only testing-mode extras `scripts/tv_uat_suite.sh` (and
 * `scripts/tv_e2e_suite.sh` before it) arms via adb — see
 * `MainActivity.EXTRA_ENABLE_TESTING_MODE` /
 * `MainActivity.EXTRA_TESTING_TOKEN`.
 *
 * The token comes from `-e TESTING_TOKEN <value>` instrumentation
 * arguments so the orchestration script can supply the same per-run,
 * host-only-file-backed token it already generates for the frozen suite.
 * Running a single test straight from Android Studio (no orchestration
 * script) falls back to a fixed local token; that only satisfies the
 * media server's debug `begin_testing` check when the server itself
 * was started with a matching control file — see `swarm-local-testing`
 * for the manual Testing-tab flow when no control file exists.
 */
abstract class UatTestBase {

    private val instrumentationArgs = InstrumentationRegistry.getArguments()

    protected val testingToken: String =
        instrumentationArgs.getString("TESTING_TOKEN") ?: "uat-local-run-token"

    private val launchIntent: Intent
        get() = Intent(
            ApplicationProvider.getApplicationContext(),
            MainActivity::class.java,
        ).apply {
            putExtra(EXTRA_ENABLE_TESTING_MODE, true)
            putExtra(EXTRA_TESTING_TOKEN, testingToken)
        }

    /**
     * Launches the real [MainActivity] with the testing-mode Intent above.
     * `createEmptyComposeRule()` (rather than `createAndroidComposeRule`,
     * which only supports launching with the default no-extras Intent) is
     * the documented pattern for exercising a Compose hierarchy behind a
     * custom-Intent activity launch — it attaches to whichever Activity is
     * currently resumed rather than launching one itself.
     */
    private val activityScenarioRule = ActivityScenarioRule<MainActivity>(launchIntent)

    val composeTestRule = createEmptyComposeRule()

    private val failureCaptureRule = UatFailureCaptureRule(composeTestRuleProvider = { composeTestRule })

    @get:Rule
    val ruleChain: TestRule = RuleChain
        .outerRule(failureCaptureRule)
        .around(activityScenarioRule)
        .around(composeTestRule)

    protected val device: UiDevice by lazy {
        UiDevice.getInstance(InstrumentationRegistry.getInstrumentation())
    }

    /** Real, in-process access to the same Room DB the app itself uses — see AppDatabase.getInstance. */
    protected val appDatabase: AppDatabase by lazy {
        AppDatabase.getInstance(InstrumentationRegistry.getInstrumentation().targetContext)
    }

    /**
     * Real, in-process access to the app's SharedPreferences-backed stores
     * (watchlist/continue-watching/likes are NOT in Room — see store.rs /
     * AppDatabase doc comments and the plan's flagged finding #4). These
     * are plain `Context`-constructed wrappers, so a fresh instance here
     * reads/writes the exact same backing file the app under test uses.
     */
    protected val likedEntriesStore: AndroidLikedEntriesStore by lazy {
        AndroidLikedEntriesStore(InstrumentationRegistry.getInstrumentation().targetContext)
    }
    protected val watchlistStore: AndroidWatchlistStore by lazy {
        AndroidWatchlistStore(InstrumentationRegistry.getInstrumentation().targetContext)
    }
    protected val watchStateStore: AndroidWatchStateStore by lazy {
        AndroidWatchStateStore(InstrumentationRegistry.getInstrumentation().targetContext)
    }

    @Before
    fun waitForIdleBeforeEachTest() {
        device.waitForIdle()
    }

    /**
     * Real pairing + catalog fetch after launch can take up to ~40s on real
     * hardware — the same window `tv_e2e_suite.sh`'s `lan_closed_loop_catalog`
     * test case already allows for this exact process. Gate every scenario
     * on the catalog screen genuinely being up (via [UatTestTags.FILTER_RAIL],
     * present regardless of what media the library actually has) before its
     * body runs, rather than each test's own first `waitForTag` racing that
     * window on a much shorter default timeout.
     */
    @Before
    fun waitForCatalogReady() {
        waitForTag(UatTestTags.FILTER_RAIL, timeoutMs = 45_000)
    }

    protected fun pressSelect() = device.pressDPadCenter()

    protected fun pressBack() = device.pressBack()

    protected fun pressDpadUp() = device.pressDPadUp()
    protected fun pressDpadDown() = device.pressDPadDown()
    protected fun pressDpadLeft() = device.pressDPadLeft()
    protected fun pressDpadRight() = device.pressDPadRight()

    /**
     * Polls Compose semantics for a node carrying [tag], failing the test if
     * it never appears. Tolerates `IllegalStateException: No compose
     * hierarchies found in the app` — the real, transient window between
     * `MainActivity` launching and its first `setContent()` registering with
     * the test framework — by retrying instead of letting it abort the poll
     * on its first check (see `UatMatchers.kt`'s doc comment for the full
     * story; this method has its own copy of the same tolerance since it
     * queries by exact tag rather than going through those helpers).
     */
    protected fun waitForTag(tag: String, timeoutMs: Long = 5_000) {
        composeTestRule.waitUntil(timeoutMillis = timeoutMs) {
            try {
                composeTestRule.onAllNodesWithTag(tag).fetchSemanticsNodes().isNotEmpty()
            } catch (e: IllegalStateException) {
                false
            }
        }
    }

    /** UIAutomator fallback for content that has left Compose's tree (e.g. after a navigation transition). */
    protected fun waitForText(text: String, timeoutMs: Long = 5_000) {
        device.wait(Until.hasObject(By.textContains(text)), timeoutMs)
    }

    private companion object {
        // Mirrors the private constants in MainActivity.kt — kept in sync
        // manually since those are `private const val` there by design
        // (see MainActivity.kt's own doc comment on why testing-mode
        // extras aren't part of any public API surface).
        const val EXTRA_ENABLE_TESTING_MODE = "app.swarm.tv.extra.ENABLE_TESTING_MODE"
        const val EXTRA_TESTING_TOKEN = "app.swarm.tv.extra.TESTING_TOKEN"
    }
}
