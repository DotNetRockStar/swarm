package app.swarm.tv.app.update

import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.core.content.FileProvider
import java.io.File

/**
 * Hands a downloaded, checksum-verified APK to the system package installer.
 * On Fire TV "Apps from Unknown Sources" is already enabled (the app was
 * sideloaded), so this shows the standard confirm-install screen. The APK is
 * signed with the same certificate as the running build, so it installs as an
 * update rather than requiring an uninstall.
 */
object ApkInstaller {
    fun install(context: Context, apk: File) {
        val uri: Uri = FileProvider.getUriForFile(
            context,
            "${context.packageName}.updates",
            apk,
        )
        val intent = Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(uri, "application/vnd.android.package-archive")
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        context.startActivity(intent)
    }

    /** Where [app.swarm.tv.core.update.UpdateChecker.download] should write. */
    fun stagingFile(context: Context, versionCode: Long): File =
        File(context.cacheDir, "updates/swarm-tv-$versionCode.apk")
}
