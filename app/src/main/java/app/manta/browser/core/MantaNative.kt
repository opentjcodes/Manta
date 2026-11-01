package app.manta.browser.core

/**
 * Direct Zero-Overhead JNI Bridge to `libmanta_core.so` (Rust).
 */
object MantaNative {
    init {
        System.loadLibrary("manta_core")
    }

    @JvmStatic
    external fun initEngine(devCorsPermissive: Boolean): Int

    @JvmStatic
    external fun createNewTab(): Int

    @JvmStatic
    external fun switchTab(tabId: Int): Boolean

    @JvmStatic
    external fun setCorsDevMode(enabled: Boolean)

    @JvmStatic
    external fun streamHtmlChunk(
        tabId: Int,
        url: String,
        htmlChunk: String,
        isEof: Boolean
    ): String

    @JvmStatic
    external fun sanitizeAndCheckUrl(url: String): String

    @JvmStatic
    external fun copyToClipboard(text: String): Boolean

    @JvmStatic
    external fun getClipboardText(): String

    @JvmStatic
    external fun closeTabAndZeroize(tabId: Int)

    @JvmStatic
    external fun purgeAllVolatileRam()

    @JvmStatic
    external fun getEngineRamBytes(): Int
}
