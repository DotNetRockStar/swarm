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
    fun `music maps D-pad directions and transport buttons to seeking`() {
        assertEquals(RemotePlaybackAction.SEEK_BACK, musicPlaybackAction(KeyEvent.KEYCODE_DPAD_LEFT))
        assertEquals(RemotePlaybackAction.SEEK_FORWARD, musicPlaybackAction(KeyEvent.KEYCODE_DPAD_RIGHT))
        assertEquals(RemotePlaybackAction.SEEK_BACK, musicPlaybackAction(KeyEvent.KEYCODE_MEDIA_REWIND))
        assertEquals(RemotePlaybackAction.SEEK_FORWARD, musicPlaybackAction(KeyEvent.KEYCODE_MEDIA_FAST_FORWARD))
    }
}
