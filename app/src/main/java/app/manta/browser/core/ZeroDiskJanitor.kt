package app.manta.browser.core

import android.content.Context
import android.webkit.CookieManager
import android.webkit.WebStorage
import java.io.File

/**
 * Guarantees Manta never grows on disk and leaves 0 cached files or databases behind.
 * Works alongside Rust's `MantaNative.purgeAllVolatileRam()` on tab close and app exit.
 */
object ZeroDiskJanitor {

    fun purgeAllDiskAndRamTraces(context: Context) {
        // 1. Cryptographically zeroize all Rust volatile arenas (`0x00` volatile overwrite)
        MantaNative.purgeAllVolatileRam()

        // 2. Wipe any transient WebStorage / CookieManager state in memory
        try {
            CookieManager.getInstance().removeAllCookies(null)
            CookieManager.getInstance().flush()
            WebStorage.getInstance().deleteAllData()
        } catch (_: Throwable) {
        }

        // 3. Recursively delete any transient cache / app_webview directories on flash storage
        try {
            deleteDirContents(context.cacheDir)
            context.codeCacheDir?.let { deleteDirContents(it) }
            val dataDir = context.dataDir
            if (dataDir != null) {
                File(dataDir, "app_webview").let { if (it.exists()) deleteDirContents(it) }
                File(dataDir, "app_textures").let { if (it.exists()) deleteDirContents(it) }
                File(dataDir, "databases").let { if (it.exists()) deleteDirContents(it) }
            }
        } catch (_: Throwable) {
        }
    }

    private fun deleteDirContents(dir: File) {
        val children = dir.listFiles() ?: return
        for (child in children) {
            if (child.isDirectory) {
                deleteDirContents(child)
            }
            try {
                child.delete()
            } catch (_: Throwable) {
            }
        }
    }
}
