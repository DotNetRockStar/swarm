package app.swarm.tv.app.uat

import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.junit4.ComposeTestRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onRoot

/**
 * Shared query helpers for scenario tests. Card/row tiles are tagged with a
 * shared prefix plus a per-entry key (see [app.swarm.tv.app.ui.UatTestTags]),
 * since a fixed shelf can hold an unbounded, content-dependent number of
 * entries — these let a test pick "some" entry without hardcoding a key.
 *
 * Built on a manual semantics-tree walk rather than `onAllNodes(matcher)`:
 * this repo's Compose Test dependency resolves `onNodeWithTag`/`onRoot` as
 * top-level extensions but not the generic matcher-based `onAllNodes`, so a
 * plain recursive walk from the root is the more portable choice here.
 *
 * Every query below tolerates `onRoot()` throwing `IllegalStateException:
 * No compose hierarchies found in the app` and treats it as "nothing found
 * yet" rather than letting it escape — that exception is expected and
 * transient in the real window between `MainActivity` launching and its
 * first `setContent()` actually registering with the test framework (a real
 * gap on real hardware, not a bug in the app). Without this, the very first
 * semantics query in any test aborts `waitUntil`'s retry loop instantly
 * instead of retrying, which is exactly what "every scenario fails within a
 * few seconds of the app opening, no compose hierarchies found" turned out
 * to be — confirmed against a real device before this fix landed.
 */

private fun SemanticsNode.testTagOrNull(): String? =
    config.firstOrNull { it.key == SemanticsProperties.TestTag }?.value as? String

private fun SemanticsNode.isFocused(): Boolean =
    config.firstOrNull { it.key == SemanticsProperties.Focused }?.value == true

private fun collectNodes(node: SemanticsNode, into: MutableList<SemanticsNode>) {
    into.add(node)
    node.children.forEach { collectNodes(it, into) }
}

private fun ComposeTestRule.allSemanticsNodes(): List<SemanticsNode> = try {
    val all = mutableListOf<SemanticsNode>()
    collectNodes(onRoot(useUnmergedTree = true).fetchSemanticsNode(), all)
    all
} catch (e: IllegalStateException) {
    emptyList()
}

/** Every distinct tag currently rendered that starts with [prefix] (e.g. one per episode/track row right now). */
fun ComposeTestRule.allTagsStartingWith(prefix: String): List<String> =
    allSemanticsNodes().mapNotNull { it.testTagOrNull() }.filter { it.startsWith(prefix) }.distinct()

/** The tag string (prefix + entry key) of the first matching node, or null if none exist yet. */
fun ComposeTestRule.firstTagStartingWith(prefix: String): String? = allTagsStartingWith(prefix).firstOrNull()

/** The matching tag that currently owns real TV focus, if focus restoration has settled. */
fun ComposeTestRule.focusedTagStartingWith(prefix: String): String? =
    allSemanticsNodes().firstOrNull { it.isFocused() && it.testTagOrNull()?.startsWith(prefix) == true }?.testTagOrNull()

/** How many currently-rendered nodes have a tag starting with [prefix]. */
fun ComposeTestRule.countNodesWithTagPrefix(prefix: String): Int = allTagsStartingWith(prefix).size

/** How many distinct matching tags are rendered beneath the exact tagged parent. */
fun ComposeTestRule.countDescendantNodesWithTagPrefix(parentTag: String, prefix: String): Int {
    val parent = allSemanticsNodes().firstOrNull { it.testTagOrNull() == parentTag } ?: return 0
    val descendants = mutableListOf<SemanticsNode>()
    parent.children.forEach { collectNodes(it, descendants) }
    return descendants.mapNotNull { it.testTagOrNull() }.filter { it.startsWith(prefix) }.distinct().size
}

/**
 * Polls until at least one node's tag starts with [prefix] — the
 * prefix-aware counterpart to [UatTestBase.waitForTag], which only matches
 * an exact tag and so cannot be used for a per-entry-suffixed prefix
 * constant (card/row tags, e.g. `EPISODE_ITEM_PREFIX`).
 */
fun ComposeTestRule.waitForTagPrefix(prefix: String, timeoutMs: Long = 5_000) {
    waitUntil(timeoutMillis = timeoutMs) { allTagsStartingWith(prefix).isNotEmpty() }
}

/**
 * All visible text under the node tagged [tag], concatenated — used where a
 * scenario needs to read displayed content (an episode's number/name, a
 * track's title) but the individual pieces aren't each given their own
 * testTag. Best-effort: walks the merged semantics subtree collecting every
 * [SemanticsProperties.Text] value (an `AnnotatedString` list; `toString()`
 * on each yields its plain text).
 */
fun ComposeTestRule.textUnderTag(tag: String): String = try {
    val root = onNodeWithTag(tag, useUnmergedTree = true).fetchSemanticsNode()
    val pieces = mutableListOf<String>()
    fun collect(node: SemanticsNode) {
        (node.config.firstOrNull { it.key == SemanticsProperties.Text }?.value as? List<*>)
            ?.forEach { pieces.add(it.toString()) }
        node.children.forEach { collect(it) }
    }
    collect(root)
    pieces.joinToString(" ")
} catch (e: IllegalStateException) {
    ""
}
