package com.smepro.app

import android.app.Activity
import android.app.Application
import android.content.Intent
import android.os.Bundle
import androidx.core.content.ContextCompat
import androidx.core.view.WindowCompat

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
        // THE GAP THIS CLOSES: env(safe-area-inset-bottom)/-top/-left/
        // -right in the frontend's CSS (see index.css, mobile.css, and
        // the fixed-position elements in AiFloatingButton.tsx,
        // UpdateChecker.tsx, AndroidUpdateChecker.tsx) all silently
        // resolve to 0 on Android unless the WebView's window is
        // explicitly drawing edge-to-edge — unlike iOS's WKWebView,
        // which reports real safe-area insets automatically. The
        // frontend already assumed this would just work (it does on
        // iOS/desktop) and every affected element already has correct
        // CSS math around it; the only missing piece was ever this one
        // line of native setup. index.html already has the matching
        // `viewport-fit=cover` meta tag this depends on.
        //
        // Same reasoning as the foreground-service wiring above for
        // WHY this lives here instead of patching MainActivity.kt
        // directly: Application.registerActivityLifecycleCallbacks
        // reaches every Activity Tauri's generated project creates —
        // including its auto-generated MainActivity — without this
        // repo ever needing to touch, patch, or even know the exact
        // shape of that generated file.
        //
        // NOT verified against a real device/emulator — this sandbox
        // has neither. What IS confirmed: WindowCompat.
        // setDecorFitsSystemWindows(window, false) is the standard,
        // documented AndroidX call for opting an Activity's window
        // into edge-to-edge drawing, and Chromium's WebView (the
        // engine Tauri's Android WebView is built on) has mapped
        // real system-bar insets to CSS env(safe-area-inset-*) once a
        // window draws edge-to-edge since well before this project's
        // minimum supported Android/WebView version.
        registerActivityLifecycleCallbacks(EdgeToEdgeInsetForwarder())
    }
}

private class EdgeToEdgeInsetForwarder : Application.ActivityLifecycleCallbacks {
    override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {
        WindowCompat.setDecorFitsSystemWindows(activity.window, false)
    }
    override fun onActivityStarted(activity: Activity) {}
    override fun onActivityResumed(activity: Activity) {}
    override fun onActivityPaused(activity: Activity) {}
    override fun onActivityStopped(activity: Activity) {}
    override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}
    override fun onActivityDestroyed(activity: Activity) {}
}

