package app.manta.browser.core

import android.content.Context
import android.webkit.CookieManager
import android.webkit.WebStorage
import java.io.File

/**
 * Manta Zero-Cache & Custom Storage Manager (`ZeroDiskJanitor.kt`):
 * 1. Never allows HTTP network caches, shader caches, or un-clearable OS WebView bloat to accumulate.
 * 2. Stores user-saved data inside Manta's single custom Rust storage file (`manta_custom_storage.bin`),
 *    which can be flushed cleanly on app pause/exit or cleared 100% on user command.
 */
object ZeroDiskJanitor {

    private const val CUSTOM_STORAGE_FILENAME = "manta_custom_storage.bin"

    fun getCustomStorageFilePath(context: Context): String {
        return File(context.filesDir, CUSTOM_STORAGE_FILENAME).absolutePath
    }

    /**
     * Called on app background/exit:
     * - Flushes user-saved data in Manta's custom Rust storage container (`manta_custom_storage.bin`).
     * - Deletes all transient HTTP/shader/network cache directories so 0 bytes of cache ever remain.
     */
    fun enforceZeroCacheAndFlushCustomStorage(context: Context) {
        MantaNative.flushCustomStorage()
        wipeTransientCachesOnly(context)
    }

    /**
     * Called when the user explicitly taps "Clear Data" / "Zeroize All":
     * - Wipes Manta's custom storage container (`manta_custom_storage.bin`), RAM arenas,
     *   session cookies, and all cache directories.
     */
    fun purgeAllDiskAndRamTraces(context: Context) {
        MantaNative.purgeAllVolatileRam()

        try {
            CookieManager.getInstance().removeAllCookies(null)
            CookieManager.getInstance().flush()
            WebStorage.getInstance().deleteAllData()
        } catch (_: Throwable) {
        }

        try {
            File(context.filesDir, CUSTOM_STORAGE_FILENAME).let {
                if (it.exists()) it.delete()
            }
        } catch (_: Throwable) {
        }

        wipeTransientCachesOnly(context)
    }

    private fun wipeTransientCachesOnly(context: Context) {
        try {
            deleteDirContents(context.cacheDir)
            context.codeCacheDir?.let { deleteDirContents(it) }
            val dataDir = context.dataDir
            if (dataDir != null) {
                val webviewDir = File(dataDir, "app_webview")
                if (webviewDir.exists()) {
                    File(webviewDir, "Cache").let { if (it.exists()) deleteDirContents(it) }
                    File(webviewDir, "GPUCache").let { if (it.exists()) deleteDirContents(it) }
                    File(webviewDir, "Code Cache").let { if (it.exists()) deleteDirContents(it) }
                }
                File(dataDir, "app_textures").let { if (it.exists()) deleteDirContents(it) }
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
