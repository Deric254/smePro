package com.smepro.app

import android.app.Activity
import android.content.Intent
import androidx.core.content.FileProvider
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin
import java.io.File

@InvokeArg
class InstallApkArgs {
    lateinit var path: String
}

/**
 * Registered from Rust in installer.rs (see that file's doc comment
 * for the full "why does this exist" story) and copied into the
 * generated Android project the same way SmeProApplication.kt and
 * SmeProForegroundService.kt already are (see mobile-android.sh and
 * release.yml's "Wiring in SmeProApplication" step — this file rides
 * along with that same copy step).
 *
 * `@tauri-apps/plugin-opener`'s `openPath()` cannot hand a downloaded
 * APK to Android's package installer: it passes a raw `file://` URI,
 * and Android has refused to let one app expose a raw file:// URI to
 * another since API 24 (Nougat) — the installer then simply has no
 * activity able to open it. The fix is the standard Android pattern
 * for exactly this (self-installing an APK via
 * REQUEST_INSTALL_PACKAGES): wrap the file in a `content://` URI
 * through our own FileProvider (declared in AndroidManifest.xml,
 * authority `${applicationId}.fileprovider` — see the "Adding
 * FileProvider" step alongside this file's copy step, and
 * res/xml/file_paths.xml) before handing it to the installer.
 *
 * REQUEST_INSTALL_PACKAGES itself (also added by that same step) is
 * what lets Android show its "Install unknown apps" / "Update this
 * app?" confirmation on top of this — that OS confirmation is a real
 * security requirement for any app not installed through the Play
 * Store, not something this plugin tries to skip.
 */
@TauriPlugin
class InstallerPlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun installApk(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(InstallApkArgs::class.java)
            val file = File(args.path)
            val uri = FileProvider.getUriForFile(activity, "${activity.packageName}.fileprovider", file)
            val intent = Intent(Intent.ACTION_VIEW).apply {
                setDataAndType(uri, "application/vnd.android.package-archive")
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
            activity.startActivity(intent)
            invoke.resolve()
        } catch (e: Exception) {
            // Surfaced to the frontend's catch block in
            // AndroidUpdateChecker.tsx as e.message, same as every
            // other failure in that flow — not a raw stack trace, but
            // not a swallowed silent failure either.
            invoke.reject(e.message ?: "Could not start the installer")
        }
    }
}
