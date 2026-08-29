package com.smepro.app

import android.app.Application
import android.content.Intent
import androidx.core.content.ContextCompat

/**
 * Custom Application subclass, wired in via
 * `android:name=".SmeProApplication"` on the generated manifest's
 * `<application>` tag (see the CI patch step in .github/workflows/release.yml
 * that adds this attribute after `tauri android init` scaffolds the
 * project). Chosen over patching Tauri's generated MainActivity.kt
 * directly because that file's exact template content isn't something
 * this repo controls or can safely regex-patch across Tauri versions —
 * this file is entirely our own, so wiring it in is a single,
 * predictable manifest attribute instead of a fragile text patch.
 *
 * Starts SmeProForegroundService as early as possible in the process's
 * lifetime — before any Activity even exists — so the background
 * execution protection is in place from the moment the app launches,
 * not only after the user first opens some particular screen.
 */
class SmeProApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        ContextCompat.startForegroundService(
            this,
            Intent(this, SmeProForegroundService::class.java)
        )
    }
}
