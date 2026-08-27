package app.swarm.tv.app.ui.screens

import android.view.KeyEvent
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Test

class PlayerRemoteInputTest {
    @Test
    fun `Fire TV transport buttons map to playback actions`() {
        assertEquals(RemotePlaybackAction.TOGGLE_PLAY_PAUSE, remotePlaybackAction(KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE))
        assertEquals(RemotePlaybackAction.PLAY, remotePlaybackAction(KeyEvent.KEYCODE_MEDIA_PLAY))
        assertEquals(RemotePlaybackAction.PAUSE, remotePlaybackAction(KeyEvent.KEYCODE_MEDIA_PAUSE))
        assertEquals(RemotePlaybackAction.SEEK_FORWARD, remotePlaybackAction(KeyEvent.KEYCODE_MEDIA_FAST_FORWARD))
        assertEquals(RemotePlaybackAction.SEEK_BACK, remotePlaybackAction(KeyEvent.KEYCODE_MEDIA_REWIND))
    }

    @Test
    fun `menu variants reveal playback controls`() {
        assertEquals(RemotePlaybackAction.SHOW_CONTROLS, remotePlaybackAction(KeyEvent.KEYCODE_MENU))
        assertEquals(RemotePlaybackAction.SHOW_CONTROLS, remotePlaybackAction(KeyEvent.KEYCODE_SETTINGS))
        assertEquals(RemotePlaybackAction.SHOW_CONTROLS, remotePlaybackAction(KeyEvent.KEYCODE_MEDIA_TOP_MENU))
    }

    @Test
    fun `D-pad select remains delegated to PlayerView`() {
        assertNull(remotePlaybackAction(KeyEvent.KEYCODE_DPAD_CENTER))
        assertNull(remotePlaybackAction(KeyEvent.KEYCODE_ENTER))
    }

    @Test
    fun `bare playing video maps D-pad seek and select shortcuts`() {
        assertEquals(
            RemotePlaybackAction.SEEK_BACK,
            videoSurfacePlaybackAction(KeyEvent.KEYCODE_DPAD_LEFT, playWhenReady = true, controlsVisible = false),
        )
        assertEquals(
            RemotePlaybackAction.SEEK_FORWARD,
            videoSurfacePlaybackAction(KeyEvent.KEYCODE_DPAD_RIGHT, playWhenReady = true, controlsVisible = false),
        )
        assertEquals(
            RemotePlaybackAction.PAUSE,
            videoSurfacePlaybackAction(KeyEvent.KEYCODE_DPAD_CENTER, playWhenReady = true, controlsVisible = false),
        )
        assertEquals(
            RemotePlaybackAction.PAUSE,
            videoSurfacePlaybackAction(KeyEvent.KEYCODE_ENTER, playWhenReady = true, controlsVisible = false),
        )
    }

    @Test
    fun `focused skip-intro button claims the D-pad instead of pausing or seeking`() {
        assertNull(
            videoSurfacePlaybackAction(
                KeyEvent.KEYCODE_DPAD_CENTER,
                playWhenReady = true,
                controlsVisible = false,
                surfaceButtonFocused = true,
            ),
        )
        assertNull(
            videoSurfacePlaybackAction(
                KeyEvent.KEYCODE_ENTER,
                playWhenReady = true,
                controlsVisible = false,
                surfaceButtonFocused = true,
            ),
        )
        assertNull(
            videoSurfacePlaybackAction(
                KeyEvent.KEYCODE_DPAD_LEFT,
                playWhenReady = true,
                controlsVisible = false,
                surfaceButtonFocused = true,
            ),
        )
        assertNull(
            videoSurfacePlaybackAction(
                KeyEvent.KEYCODE_DPAD_RIGHT,
                playWhenReady = true,
                controlsVisible = false,
                surfaceButtonFocused = true,
            ),
        )
    }

    @Test
    fun `paused video and visible controls retain normal D-pad behavior`() {
        assertNull(videoSurfacePlaybackAction(KeyEvent.KEYCODE_DPAD_LEFT, playWhenReady = false, controlsVisible = false))
        assertNull(videoSurfacePlaybackAction(KeyEvent.KEYCODE_DPAD_RIGHT, playWhenReady = false, controlsVisible = false))
        assertNull(videoSurfacePlaybackAction(KeyEvent.KEYCODE_DPAD_CENTER, playWhenReady = false, controlsVisible = false))
        assertNull(videoSurfacePlaybackAction(KeyEvent.KEYCODE_DPAD_LEFT, playWhenReady = true, controlsVisible = true))
        assertNull(videoSurfacePlaybackAction(KeyEvent.KEYCODE_DPAD_CENTER, playWhenReady = true, controlsVisible = true))
    }

    @Test
    fun `Back dismisses playback controls before pausing or exiting`() {
        assertEquals(
            PlaybackBackAction.HIDE_CONTROLS,
            playbackBackAction(showPauseOverlay = false, showContinuePrompt = false, controlsVisible = true),
        )
        assertEquals(
            PlaybackBackAction.PAUSE,
            playbackBackAction(showPauseOverlay = false, showContinuePrompt = false, controlsVisible = false),
        )
        assertEquals(
            PlaybackBackAction.EXIT,
            playbackBackAction(showPauseOverlay = true, showContinuePrompt = false, controlsVisible = false),
        )
        assertEquals(
            PlaybackBackAction.EXIT,
            playbackBackAction(showPauseOverlay = false, showContinuePrompt = true, controlsVisible = true),
        )
    }

    @Test
    fun `credits-triggered continue prompt dismisses instead of exiting`() {
        assertEquals(
            PlaybackBackAction.DISMISS_CONTINUE_PROMPT,
            playbackBackAction(
                showPauseOverlay = false,
                showContinuePrompt = true,
                controlsVisible = true,
                continuePromptDismissable = true,
            ),
        )
        // Without an active continue prompt, the dismissable flag is moot.
        assertEquals(
            PlaybackBackAction.PAUSE,
            playbackBackAction(
                showPauseOverlay = false,
                showContinuePrompt = false,
                controlsVisible = false,
                continuePromptDismissable = true,
            ),
        )
    }

    @Test
    fun `D-pad left and right are never a playback action`() {
        // The music screen relies on remotePlaybackAction returning null for
        // the D-pad so Compose keeps left/right for focus navigation instead
        // of fast-forward / rewind (#96); video's D-pad seek lives in
        // videoSurfacePlaybackAction, not here.
        assertNull(remotePlaybackAction(KeyEvent.KEYCODE_DPAD_LEFT))
        assertNull(remotePlaybackAction(KeyEvent.KEYCODE_DPAD_RIGHT))
    }
}
