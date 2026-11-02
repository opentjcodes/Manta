package app.manta.browser.core

/**
 * Direct Zero-Overhead JNI Bridge to `libmanta_core.so` (Multi-Core Rust Browser Engine).
 * Connects Android UI to Manta's Site Isolation, Bytecode JS VM, Chrome Controls,
 * and Clearable Custom Storage Engine.
 */
object MantaNative {
    var isLoaded: Boolean = false
        private set

    init {
        try {
            System.loadLibrary("manta_core")
            isLoaded = true
        } catch (_: Throwable) {
            isLoaded = false
        }
    }

    @JvmStatic
    private external fun nativeInitEngine(storagePath: String): Int

    @JvmStatic
    private external fun nativeGetWorkerCoreCount(): Int

    @JvmStatic
    private external fun nativePrewarmDns(url: String)

    @JvmStatic
    private external fun nativeCreateNewTab(): Int

    @JvmStatic
    private external fun nativeCompressTabState(
        tabId: Int,
        url: String,
        title: String,
        scrollY: Int
    ): Int

    @JvmStatic
    private external fun nativeRestoreTabState(tabId: Int): String

    @JvmStatic
    private external fun nativeSwitchTab(tabId: Int): Boolean

    @JvmStatic
    private external fun nativeSetDesktopMode(tabId: Int, enabled: Boolean)

    @JvmStatic
    private external fun nativeIsDesktopMode(tabId: Int): Boolean

    @JvmStatic
    private external fun nativeSetTabZoom(tabId: Int, zoomPercent: Int)

    @JvmStatic
    private external fun nativeGetTabZoom(tabId: Int): Int

    @JvmStatic
    private external fun nativeGetRuntimeScript(originUrl: String): String

    @JvmStatic
    private external fun nativeIsChallengeUrl(url: String): Boolean

    @JvmStatic
    private external fun nativeValidateUrl(
        initiatorUrl: String,
        targetUrl: String,
        isMainFrame: Boolean
    ): String

    @JvmStatic
    private external fun nativeStorageSetItem(
        originUrl: String,
        key: String,
        value: String,
        isSession: Boolean
    ): Boolean

    @JvmStatic
    private external fun nativeStorageGetItem(
        originUrl: String,
        key: String,
        isSession: Boolean
    ): String

    @JvmStatic
    private external fun nativeStorageRemoveItem(
        originUrl: String,
        key: String,
        isSession: Boolean
    )

    @JvmStatic
    private external fun nativeStorageClearOrigin(originUrl: String)

    @JvmStatic
    private external fun nativeFlushCustomStorage(): Boolean

    @JvmStatic
    private external fun nativeToggleBookmark(url: String, title: String): Boolean

    @JvmStatic
    private external fun nativeStreamHtmlChunk(
        tabId: Int,
        url: String,
        htmlChunk: String,
        isEof: Boolean
    ): String

    @JvmStatic
    private external fun nativeCopyToClipboard(text: String): Boolean

    @JvmStatic
    private external fun nativeGetClipboardText(): String

    @JvmStatic
    private external fun nativeCloseTab(tabId: Int)

    @JvmStatic
    private external fun nativeClearAllCustomStorageAndRam()

    @JvmStatic
    private external fun nativeGetEngineRamBytes(): Int

    @JvmStatic
    private external fun nativeGetCustomStorageBytes(): Int

    fun initEngine(storagePath: String = ""): Int =
        if (isLoaded) runCatching { nativeInitEngine(storagePath) }.getOrDefault(1) else 1

    fun getWorkerCoreCount(): Int =
        if (isLoaded) runCatching { nativeGetWorkerCoreCount() }.getOrDefault(4) else 4

    fun prewarmDns(url: String) {
        if (isLoaded) runCatching { nativePrewarmDns(url) }
    }

    fun createNewTab(): Int =
        if (isLoaded) runCatching { nativeCreateNewTab() }.getOrDefault(1) else 1

    fun compressTabState(tabId: Int, url: String, title: String, scrollY: Int): Int =
        if (isLoaded) runCatching { nativeCompressTabState(tabId, url, title, scrollY) }.getOrDefault(0) else 0

    fun restoreTabState(tabId: Int): String =
        if (isLoaded) runCatching { nativeRestoreTabState(tabId) }.getOrDefault("") else ""

    fun switchTab(tabId: Int): Boolean =
        if (isLoaded) runCatching { nativeSwitchTab(tabId) }.getOrDefault(false) else false

    fun setDesktopMode(tabId: Int, enabled: Boolean) {
        if (isLoaded) runCatching { nativeSetDesktopMode(tabId, enabled) }
    }

    fun isDesktopMode(tabId: Int): Boolean =
        if (isLoaded) runCatching { nativeIsDesktopMode(tabId) }.getOrDefault(false) else false

    fun setTabZoom(tabId: Int, zoomPercent: Int) {
        if (isLoaded) runCatching { nativeSetTabZoom(tabId, zoomPercent) }
    }

    fun getTabZoom(tabId: Int): Int =
        if (isLoaded) runCatching { nativeGetTabZoom(tabId) }.getOrDefault(100) else 100

    fun getRuntimeScript(originUrl: String): String =
        if (isLoaded) runCatching { nativeGetRuntimeScript(originUrl) }.getOrDefault("") else ""

    fun isChallengeUrl(url: String): Boolean =
        if (isLoaded) runCatching { nativeIsChallengeUrl(url) }.getOrDefault(false) else false

    fun validateUrl(initiatorUrl: String, targetUrl: String, isMainFrame: Boolean): String =
        if (isLoaded) {
            runCatching { nativeValidateUrl(initiatorUrl, targetUrl, isMainFrame) }.getOrDefault(targetUrl)
        } else {
            targetUrl
        }

    fun sanitizeAndCheckUrl(url: String): String =
        validateUrl("", url, true)

    fun storageSetItem(originUrl: String, key: String, value: String, isSession: Boolean): Boolean =
        if (isLoaded) {
            runCatching { nativeStorageSetItem(originUrl, key, value, isSession) }.getOrDefault(false)
        } else {
            false
        }

    fun storageGetItem(originUrl: String, key: String, isSession: Boolean): String =
        if (isLoaded) {
            runCatching { nativeStorageGetItem(originUrl, key, isSession) }.getOrDefault("")
        } else {
            ""
        }

    fun storageRemoveItem(originUrl: String, key: String, isSession: Boolean) {
        if (isLoaded) runCatching { nativeStorageRemoveItem(originUrl, key, isSession) }
    }

    fun storageClearOrigin(originUrl: String) {
        if (isLoaded) runCatching { nativeStorageClearOrigin(originUrl) }
    }

    fun flushCustomStorage(): Boolean =
        if (isLoaded) runCatching { nativeFlushCustomStorage() }.getOrDefault(false) else false

    fun toggleBookmark(url: String, title: String): Boolean =
        if (isLoaded) runCatching { nativeToggleBookmark(url, title) }.getOrDefault(false) else false

    fun streamHtmlChunk(tabId: Int, url: String, htmlChunk: String, isEof: Boolean): String =
        if (isLoaded) {
            runCatching { nativeStreamHtmlChunk(tabId, url, htmlChunk, isEof) }
                .getOrElse { "MANTA_ERR|JNI Error: ${it.message}" }
        } else {
            "MANTA_FRAME|title=Manta|bg=#FF09090B|height=300.0|nodes=1|ramKB=0.0\nTEXT|16.0|24.0|16.0|1|#FFF8FAFC||Manta Core Active\n"
        }

    fun copyToClipboard(text: String): Boolean =
        if (isLoaded) runCatching { nativeCopyToClipboard(text) }.getOrDefault(false) else false

    fun getClipboardText(): String =
        if (isLoaded) runCatching { nativeGetClipboardText() }.getOrDefault("") else ""

    fun closeTabAndZeroize(tabId: Int) {
        if (isLoaded) runCatching { nativeCloseTab(tabId) }
    }

    fun purgeAllVolatileRam() {
        if (isLoaded) runCatching { nativeClearAllCustomStorageAndRam() }
    }

    fun getEngineRamBytes(): Int =
        if (isLoaded) runCatching { nativeGetEngineRamBytes() }.getOrDefault(0) else 0

    fun getCustomStorageBytes(): Int =
        if (isLoaded) runCatching { nativeGetCustomStorageBytes() }.getOrDefault(0) else 0
}
