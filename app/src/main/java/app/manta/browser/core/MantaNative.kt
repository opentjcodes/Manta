package app.manta.browser.core

/**
 * Minimal Kotlin JNI Bridge to `libmanta_core.so` (Rust).
 * Full Kotlin UI & Muse-style sliding bottom dock will connect here in Phase 2.
 */
object MantaNative {
    init {
        System.loadLibrary("manta_core")
    }

    @JvmStatic
    external fun initEngine(devCorsPermissive: Boolean): Int

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
    external fun closeTabAndZeroize(tabId: Int)

    @JvmStatic
    external fun purgeAllVolatileRam()

    @JvmStatic
    external fun getEngineRamBytes(): Int
}
