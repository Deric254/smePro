//! THE FIX for AndroidUpdateChecker.tsx's "downloads to 100%, then
//! fails with 'could not download or install the update'" bug.
//!
//! The download itself was never the problem — by the time that error
//! appears, every byte of the APK is already on disk. The failure was
//! the handoff to Android's own package installer immediately after,
//! via `@tauri-apps/plugin-opener`'s `openPath()`. That plugin's
//! Android implementation hands the installer a raw `file://` path,
//! which Android has refused to let one app pass to another since API
//! 24 (Nougat) — doing so throws a `FileUriExposedException` rather
//! than opening anything. This isn't a version we can just upgrade our
//! way out of: it's a still-open upstream limitation (see
//! https://github.com/tauri-apps/plugins-workspace/issues/2383,
//! "`opener` can only open URLs", confirmed still unresolved at time
//! of writing), not a bug that's been fixed in a newer opener release
//! we're missing.
//!
//! The fix is the standard FileProvider handoff every Android app that
//! self-installs an APK has to do: wrap the downloaded file in a
//! `content://` URI scoped to our OWN FileProvider (declared in
//! AndroidManifest.xml — see mobile-android.sh's and release.yml's
//! "Adding FileProvider" step, and res/xml/file_paths.xml) before
//! handing it to Android's package installer. `openPath()` can't do
//! this for us, so this is a small, purpose-built plugin instead of a
//! workaround bolted onto the existing opener call — see
//! InstallerPlugin.kt (in src-tauri/android/com/smepro/app/, copied
//! into the generated Android project the same way
//! SmeProApplication.kt already is) for the actual FileProvider/Intent
//! code, which is the textbook Android pattern for this — not
//! invented here, just wired in as a Tauri command.
//!
//! `install_apk` below is registered as a REGULAR app command (see
//! lib.rs's `#[cfg(not(desktop))]` invoke_handler list), not exposed
//! under a `plugin:installer|...` namespace the frontend calls
//! directly — that would need its own permission manifest (a
//! `permissions/` schema + a capabilities entry, the way every
//! separately-published plugin already used in this app, e.g.
//! tauri-plugin-http, has). Routing JS -> this ordinary command ->
//! the Kotlin plugin underneath instead means it's covered by the
//! `core:default` permission this app's capabilities/default.json
//! already grants, same as get_network_mode and every other
//! first-party command — one less place to get a permission scope
//! wrong for a fix this narrow.
//!
//! Not gated to `target_os = "android"` specifically because this
//! codebase has no iOS build at all (see lib.rs's `pub mod installer`
//! declaration for why "not desktop" already means "Android" in
//! practice throughout this app).

use tauri::{
    plugin::{Builder, PluginHandle, TauriPlugin},
    AppHandle, Manager, Runtime,
};

struct InstallerHandle<R: Runtime>(PluginHandle<R>);

#[derive(serde::Serialize)]
struct InstallApkArgs<'a> {
    path: &'a str,
}

/// Called from AndroidUpdateChecker.tsx once the APK has finished
/// downloading. `path` is the absolute path writeFile() just wrote it
/// to (under appCacheDir()) — this hands that off to
/// InstallerPlugin.kt, which does the actual FileProvider/Intent work
/// (see this module's doc comment for why that step can't be done via
/// openPath() instead).
#[tauri::command]
pub fn install_apk<R: Runtime>(app: AppHandle<R>, path: String) -> Result<(), String> {
    let handle = app.state::<InstallerHandle<R>>();
    handle
        .0
        .run_mobile_plugin::<serde_json::Value>("installApk", InstallApkArgs { path: &path })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("installer")
        .setup(|app, api| {
            let handle = api.register_android_plugin("com.smepro.app", "InstallerPlugin")?;
            app.manage(InstallerHandle(handle));
            Ok(())
        })
        .build()
}
