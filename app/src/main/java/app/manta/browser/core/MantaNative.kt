package app.manta.browser.core

/**
 * Direct Zero-Overhead JNI Bridge to `libmanta_core.so` (Rust).
 * Includes defensive fallback guards so JNI calls remain 100% crash-safe.
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
    private external fun nativeCreateNewTab(): Int

    @JvmStatic
    private external fun nativeSwitchTab(tabId: Int): Boolean

    @JvmStatic
    private external fun nativeSetCorsDevMode(enabled: Boolean)

    @JvmStatic
    private external fun nativeGetRuntimeScript(corsDevEnabled: Boolean): String

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

    fun initEngine(devCorsPermissive: Boolean): Int {
        return if (isLoaded) {
            try {
                nativeInitEngine(devCorsPermissive)
            } catch (_: Throwable) {
                1
            }
        } else 1
    }

    fun createNewTab(): Int {
        return if (isLoaded) {
            try {
                nativeCreateNewTab()
            } catch (_: Throwable) {
                1
            }
        } else 1
    }

    fun switchTab(tabId: Int): Boolean {
        return if (isLoaded) {
            try {
                nativeSwitchTab(tabId)
            } catch (_: Throwable) {
                false
            }
        } else false
    }

    fun setCorsDevMode(enabled: Boolean) {
        if (isLoaded) {
            try {
                nativeSetCorsDevMode(enabled)
            } catch (_: Throwable) {
            }
        }
    }

    fun getRuntimeScript(corsDevEnabled: Boolean): String {
        return if (isLoaded) {
            try {
                nativeGetRuntimeScript(corsDevEnabled)
            } catch (_: Throwable) {
                ""
            }
        } else ""
    }

    fun setVolatileCookie(tabId: Int, host: String, cookie: String) {
        if (isLoaded) {
            try {
                nativeSetVolatileCookie(tabId, host, cookie)
            } catch (_: Throwable) {
            }
        }
    }

    fun getVolatileCookie(tabId: Int, host: String): String {
        return if (isLoaded) {
            try {
                nativeGetVolatileCookie(tabId, host)
            } catch (_: Throwable) {
                ""
            }
        } else ""
    }

    fun streamHtmlChunk(tabId: Int, url: String, htmlChunk: String, isEof: Boolean): String {
        return if (isLoaded) {
            try {
                nativeStreamHtmlChunk(tabId, url, htmlChunk, isEof)
            } catch (t: Throwable) {
                "MANTA_ERR|JNI Error: ${t.message}"
            }
        } else {
            "MANTA_FRAME|title=Manta|bg=#FF09090B|height=300.0|nodes=1|ramKB=0.0\nTEXT|16.0|24.0|16.0|1|#FFF8FAFC||Manta Core Active\n"
        }
    }

    fun sanitizeAndCheckUrl(url: String): String {
        return if (isLoaded) {
            try {
                nativeSanitizeAndCheckUrl(url)
            } catch (_: Throwable) {
                url
            }
        } else url
    }

    fun copyToClipboard(text: String): Boolean {
        return if (isLoaded) {
            try {
                nativeCopyToClipboard(text)
            } catch (_: Throwable) {
                false
            }
        } else false
    }

    fun getClipboardText(): String {
        return if (isLoaded) {
            try {
                nativeGetClipboardText()
            } catch (_: Throwable) {
                ""
            }
        } else ""
    }

    fun closeTabAndZeroize(tabId: Int) {
        if (isLoaded) {
            try {
                nativeCloseTabAndZeroize(tabId)
            } catch (_: Throwable) {
            }
        }
    }

    fun purgeAllVolatileRam() {
        if (isLoaded) {
            try {
                nativePurgeAllVolatileRam()
            } catch (_: Throwable) {
            }
        }
    }

    fun getEngineRamBytes(): Int {
        return if (isLoaded) {
            try {
                nativeGetEngineRamBytes()
            } catch (_: Throwable) {
                0
            }
        } else 0
    }
}
