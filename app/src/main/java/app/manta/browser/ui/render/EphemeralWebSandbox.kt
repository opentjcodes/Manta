package app.manta.browser.ui.render

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Bitmap
import android.view.View
import android.webkit.CookieManager
import android.webkit.JavascriptInterface
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebSettings
import android.webkit.WebStorage
import android.webkit.WebView
import android.webkit.WebViewClient
import app.manta.browser.core.MantaNative
import app.manta.browser.ui.theme.MantaTheme
import java.io.ByteArrayInputStream

/**
 * Manta Full-Fidelity Chrome-Compatible Engine Viewport:
 * 1. Stock Chrome Compatibility & Zero Ad-Block Tampering:
 *    - Dynamically aligns `User-Agent` to the device's native Chromium engine version
 *      (supporting both Mobile and Desktop Site modes).
 *    - Enforces standard W3C Same-Origin Policy (SOP) and CORS without unsafe bypass proxies.
 * 2. Strict Per-Origin Site Isolation & SSRF Guard:
 *    - Validates subresource requests in Rust (`MantaNative.validateUrl`) to block cross-origin
 *      Private Network Access (SSRF) and unsafe schemes while allowing all legitimate site traffic.
 * 3. Custom Clearable Rust Storage Bridge (`MantaBridge`):
 *    - Routes `localStorage` and `sessionStorage` into Manta's origin-partitioned Rust storage engine
 *      while keeping WebView network cache strictly disabled (`LOAD_NO_CACHE`).
 */
@SuppressLint("SetJavaScriptEnabled", "AddJavascriptInterface")
class EphemeralWebSandbox(
    context: Context,
    val tabId: Int,
    private val isDesktopModeEnabled: () -> Boolean,
    private val onPageMetaUpdate: (tabId: Int, title: String, url: String) -> Unit,
    private val onProgressUpdate: (tabId: Int, progress: Int) -> Unit,
    private val onSecurityEvent: (String) -> Unit
) : WebView(context) {

    @Volatile
    private var currentMainFrameOriginUrl: String = ""

    inner class MantaJsBridge {
        @JavascriptInterface
        fun copyText(text: String) {
            MantaNative.copyToClipboard(text)
        }

        @JavascriptInterface
        fun storageSet(key: String, value: String, isSession: Boolean): Boolean {
            val origin = currentMainFrameOriginUrl
            if (origin.isBlank()) return false
            return MantaNative.storageSetItem(origin, key, value, isSession)
        }

        @JavascriptInterface
        fun storageGet(key: String, isSession: Boolean): String {
            val origin = currentMainFrameOriginUrl
            if (origin.isBlank()) return ""
            return MantaNative.storageGetItem(origin, key, isSession)
        }

        @JavascriptInterface
        fun storageRemove(key: String, isSession: Boolean) {
            val origin = currentMainFrameOriginUrl
            if (origin.isBlank()) return
            MantaNative.storageRemoveItem(origin, key, isSession)
        }

        @JavascriptInterface
        fun storageClearOrigin() {
            val origin = currentMainFrameOriginUrl
            if (origin.isBlank()) return
            MantaNative.storageClearOrigin(origin)
        }
    }

    init {
        setBackgroundColor(MantaTheme.BG_OBSIDIAN)
        setLayerType(View.LAYER_TYPE_HARDWARE, null)
        isFocusable = true
        isFocusableInTouchMode = true

        try {
            CookieManager.getInstance().apply {
                setAcceptCookie(true)
                setAcceptThirdPartyCookies(this@EphemeralWebSandbox, true)
            }
        } catch (_: Throwable) {
        }

        settings.apply {
            javaScriptEnabled = true
            domStorageEnabled = true
            databaseEnabled = false
            cacheMode = WebSettings.LOAD_NO_CACHE

            useWideViewPort = true
            loadWithOverviewMode = true
            loadsImagesAutomatically = true
            blockNetworkImage = false

            setSupportZoom(true)
            builtInZoomControls = true
            displayZoomControls = false
            javaScriptCanOpenWindowsAutomatically = false
            setSupportMultipleWindows(false)
            mediaPlaybackRequiresUserGesture = false
            allowFileAccess = false
            allowContentAccess = false
            mixedContentMode = WebSettings.MIXED_CONTENT_COMPATIBILITY_MODE
            userAgentString = buildCleanChromeUserAgent(context, isDesktopModeEnabled())
        }

        addJavascriptInterface(MantaJsBridge(), "MantaBridge")

        webChromeClient = object : WebChromeClient() {
            override fun onProgressChanged(view: WebView?, newProgress: Int) {
                super.onProgressChanged(view, newProgress)
                post { onProgressUpdate(tabId, newProgress) }
            }

            override fun onReceivedTitle(view: WebView?, title: String?) {
                super.onReceivedTitle(view, title)
                val currentUrl = view?.url ?: ""
                if (!title.isNullOrBlank()) {
                    post { onPageMetaUpdate(tabId, title, currentUrl) }
                }
            }
        }

        webViewClient = object : WebViewClient() {
            override fun shouldInterceptRequest(
                view: WebView,
                request: WebResourceRequest
            ): WebResourceResponse? {
                val rawUrl = request.url?.toString() ?: return null
                if (!rawUrl.startsWith("http://") && !rawUrl.startsWith("https://")) {
                    return super.shouldInterceptRequest(view, request)
                }

                if (MantaNative.isChallengeUrl(rawUrl)) {
                    return super.shouldInterceptRequest(view, request)
                }

                val initiator = if (request.isForMainFrame) "" else currentMainFrameOriginUrl
                val verdict = MantaNative.validateUrl(initiator, rawUrl, request.isForMainFrame)
                if (verdict == "BLOCKED") {
                    val host = request.url?.host ?: rawUrl
                    post { onSecurityEvent("Site Isolation Blocked SSRF/Unsafe: $host") }
                    return WebResourceResponse(
                        "text/plain",
                        "UTF-8",
                        403,
                        "Forbidden",
                        emptyMap(),
                        ByteArrayInputStream(ByteArray(0))
                    )
                }

                return super.shouldInterceptRequest(view, request)
            }

            override fun onPageStarted(view: WebView, url: String?, favicon: Bitmap?) {
                super.onPageStarted(view, url, favicon)
                if (url != null) {
                    currentMainFrameOriginUrl = url
                    if (!MantaNative.isChallengeUrl(url)) {
                        MantaNative.prewarmDns(url)
                        val runtimeJs = MantaNative.getRuntimeScript(url)
                        if (runtimeJs.isNotEmpty()) {
                            view.evaluateJavascript(runtimeJs, null)
                        }
                    }
                    post { onPageMetaUpdate(tabId, view.title ?: url, url) }
                }
            }

            override fun onPageFinished(view: WebView, url: String?) {
                super.onPageFinished(view, url)
                if (url != null) {
                    currentMainFrameOriginUrl = url
                    if (!MantaNative.isChallengeUrl(url)) {
                        val runtimeJs = MantaNative.getRuntimeScript(url)
                        if (runtimeJs.isNotEmpty()) {
                            view.evaluateJavascript(runtimeJs, null)
                        }
                    }
                    MantaNative.flushCustomStorage()
                }
                post {
                    onProgressUpdate(tabId, 100)
                    onPageMetaUpdate(tabId, view.title ?: "Manta", url ?: "")
                }
            }
        }
    }

    fun applyDesktopModeSetting(desktopEnabled: Boolean) {
        MantaNative.setDesktopMode(tabId, desktopEnabled)
        settings.userAgentString = buildCleanChromeUserAgent(context, desktopEnabled)
        settings.useWideViewPort = true
        settings.loadWithOverviewMode = true
    }

    /**
     * Pauses JS timers and rendering when this tab moves to the background to save RAM & battery.
     */
    fun hibernateInBackground() {
        try {
            onPause()
            MantaNative.compressTabState(tabId, url ?: "", title ?: "Manta Tab", scrollY)
        } catch (_: Throwable) {
        }
    }

    /**
     * Wakes the tab immediately at its exact scroll position without losing state.
     */
    fun wakeFromHibernation() {
        try {
            onResume()
        } catch (_: Throwable) {
        }
    }

    fun releaseTabResources() {
        try {
            stopLoading()
            loadUrl("about:blank")
            clearHistory()
            clearFormData()
            clearCache(false)
        } catch (_: Throwable) {
        }
    }

    fun wipeAllEphemeralData() {
        try {
            stopLoading()
            loadUrl("about:blank")
            clearHistory()
            clearFormData()
            clearCache(true)
            CookieManager.getInstance().removeAllCookies(null)
            WebStorage.getInstance().deleteAllData()
        } catch (_: Throwable) {
        }
    }

    companion object {
        /**
         * Builds a stock Chrome User-Agent matching the device's real Chromium engine version
         * (supporting both Mobile and Desktop Site modes).
         */
        fun buildCleanChromeUserAgent(context: Context, desktopMode: Boolean = false): String {
            val chromeVer = try {
                val raw = WebSettings.getDefaultUserAgent(context)
                Regex("Chrome/([0-9.]+)").find(raw)?.groupValues?.getOrNull(1) ?: "131.0.0.0"
            } catch (_: Throwable) {
                "131.0.0.0"
            }
            return if (desktopMode) {
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/$chromeVer Safari/537.36"
            } else {
                "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/$chromeVer Mobile Safari/537.36"
            }
        }
    }
}
