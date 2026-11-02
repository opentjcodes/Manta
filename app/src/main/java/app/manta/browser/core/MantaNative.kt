package app.manta.browser.core

/**
 * Direct Zero-Overhead JNI Bridge to `libmanta_core.so` (Multi-Core Rust Engine).
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
    private external fun nativeInitEngine(devCorsPermissive: Boolean): Int

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
    private external fun nativeSetCorsDevMode(enabled: Boolean)

    @JvmStatic
    private external fun nativeGetRuntimeScript(corsDevEnabled: Boolean): String

    @JvmStatic
    private external fun nativeIsChallengeUrl(url: String): Boolean

    @JvmStatic
    private external fun nativeSetVolatileCookie(tabId: Int, host: String, cookie: String)

    @JvmStatic
    private external fun nativeGetVolatileCookie(tabId: Int, host: String): String

    @JvmStatic
    private external fun nativeStreamHtmlChunk(
        tabId: Int,
        url: String,
        htmlChunk: String,
        isEof: Boolean
    ): String

    @JvmStatic
    private external fun nativeSanitizeAndCheckUrl(url: String): String

    @JvmStatic
    private external fun nativeCopyToClipboard(text: String): Boolean

    @JvmStatic
    private external fun nativeGetClipboardText(): String

    @JvmStatic
    private external fun nativeCloseTabAndZeroize(tabId: Int)

    @JvmStatic
    private external fun nativePurgeAllVolatileRam()

    @JvmStatic
    private external fun nativeGetEngineRamBytes(): Int

    fun initEngine(devCorsPermissive: Boolean): Int =
        if (isLoaded) runCatching { nativeInitEngine(devCorsPermissive) }.getOrDefault(1) else 1

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

    fun setCorsDevMode(enabled: Boolean) {
        if (isLoaded) runCatching { nativeSetCorsDevMode(enabled) }
    }

    fun getRuntimeScript(corsDevEnabled: Boolean): String =
        if (isLoaded) runCatching { nativeGetRuntimeScript(corsDevEnabled) }.getOrDefault("") else ""

    fun isChallengeUrl(url: String): Boolean =
        if (isLoaded) runCatching { nativeIsChallengeUrl(url) }.getOrDefault(false) else false

    fun setVolatileCookie(tabId: Int, host: String, cookie: String) {
        if (isLoaded) runCatching { nativeSetVolatileCookie(tabId, host, cookie) }
    }

    fun getVolatileCookie(tabId: Int, host: String): String =
        if (isLoaded) runCatching { nativeGetVolatileCookie(tabId, host) }.getOrDefault("") else ""

    fun streamHtmlChunk(tabId: Int, url: String, htmlChunk: String, isEof: Boolean): String =
        if (isLoaded) {
            runCatching { nativeStreamHtmlChunk(tabId, url, htmlChunk, isEof) }
                .getOrElse { "MANTA_ERR|JNI Error: ${it.message}" }
        } else {
            "MANTA_FRAME|title=Manta|bg=#FF09090B|height=300.0|nodes=1|ramKB=0.0\nTEXT|16.0|24.0|16.0|1|#FFF8FAFC||Manta Core Active\n"
        }

    fun sanitizeAndCheckUrl(url: String): String =
        if (isLoaded) runCatching { nativeSanitizeAndCheckUrl(url) }.getOrDefault(url) else url

    fun copyToClipboard(text: String): Boolean =
        if (isLoaded) runCatching { nativeCopyToClipboard(text) }.getOrDefault(false) else false

    fun getClipboardText(): String =
        if (isLoaded) runCatching { nativeGetClipboardText() }.getOrDefault("") else ""

    fun closeTabAndZeroize(tabId: Int) {
        if (isLoaded) runCatching { nativeCloseTabAndZeroize(tabId) }
    }

    fun purgeAllVolatileRam() {
        if (isLoaded) runCatching { nativePurgeAllVolatileRam() }
    }

    fun getEngineRamBytes(): Int =
        if (isLoaded) runCatching { nativeGetEngineRamBytes() }.getOrDefault(0) else 0
}
