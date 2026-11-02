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
import org.json.JSONObject
import java.io.ByteArrayInputStream
import java.net.HttpURLConnection
import java.net.URL

/**
 * Manta Full-Fidelity Rust-Accelerated Engine Viewport:
 * 1. Cloudflare Turnstile & CAPTCHA Ready:
 *    - Dynamically aligns `User-Agent` to the device's exact native Chromium engine version
 *      (eliminating TLS JA3 / `Sec-CH-UA` version mismatches that trigger Cloudflare blocks).
 *    - Enables session-only first & third-party cookies in memory so Turnstile (`__cf_bm` / `cf_clearance`)
 *      and reCAPTCHA/hCaptcha iframes complete verification on the first click, then wipes all cookies on close.
 *    - Leaves all `challenges.cloudflare.com`, `hcaptcha.com`, and `recaptcha.net` requests untouched.
 * 2. True Developer CORS Unlocker:
 *    - Intercepts cross-origin `OPTIONS` preflights and `GET` API requests in `shouldInterceptRequest`,
 *      echoing the exact `Origin` and `Access-Control-Request-Headers`.
 *    - Provides `MantaBridge.corsRequest(method, url, headersJson, body)` for cross-origin `POST`/`PUT`/`PATCH`/`DELETE`.
 * 3. Cookie-Banner Eliminator & Rust Ad/Tracker Shield:
 *    - Automatically dismisses GDPR/Cookie consent walls and unlocks page scrolling.
 */
@SuppressLint("SetJavaScriptEnabled", "AddJavascriptInterface")
class EphemeralWebSandbox(
    context: Context,
    val tabId: Int,
    private val isCorsDevEnabled: () -> Boolean,
    private val onPageMetaUpdate: (tabId: Int, title: String, url: String) -> Unit,
    private val onProgressUpdate: (tabId: Int, progress: Int) -> Unit,
    private val onShieldEvent: (String) -> Unit
) : WebView(context) {

    inner class MantaJsBridge {
        @JavascriptInterface
        fun copyText(text: String) {
            MantaNative.copyToClipboard(text)
        }

        /**
         * Full-featured Developer CORS Bridge for `fetch()` and `XMLHttpRequest`
         * supporting `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, custom request headers, and bodies.
         */
        @JavascriptInterface
        fun corsRequest(
            method: String,
            targetUrl: String,
            headersJson: String,
            body: String
        ): String {
            if (!isCorsDevEnabled()) {
                return JSONObject().put("error", "CORS Dev Mode Disabled").toString()
            }
            val clean = MantaNative.sanitizeAndCheckUrl(targetUrl)
            if (clean == "BLOCKED") {
                return JSONObject().put("error", "Blocked by Shield").toString()
            }
            return try {
                val upperMethod = method.uppercase().trim()
                val conn = (URL(clean).openConnection() as HttpURLConnection).apply {
                    requestMethod = upperMethod
                    connectTimeout = 8000
                    readTimeout = 10000
                    useCaches = false
                    instanceFollowRedirects = true
                    setRequestProperty("Accept", "*/*")
                    setRequestProperty("User-Agent", buildCleanChromeUserAgent(context))

                    if (headersJson.isNotBlank() && headersJson.startsWith("{")) {
                        val hdrObj = JSONObject(headersJson)
                        val keys = hdrObj.keys()
                        while (keys.hasNext()) {
                            val k = keys.next()
                            val v = hdrObj.optString(k, "")
                            if (k.isNotBlank() && v.isNotBlank()) {
                                setRequestProperty(k, v)
                            }
                        }
                    }

                    if (body.isNotEmpty() && (upperMethod == "POST" || upperMethod == "PUT" || upperMethod == "PATCH")) {
                        doOutput = true
                        if (getRequestProperty("Content-Type") == null) {
                            setRequestProperty("Content-Type", "application/json; charset=utf-8")
                        }
                        outputStream.use { it.write(body.toByteArray(Charsets.UTF_8)) }
                    }
                }

                val status = conn.responseCode
                val contentType = (conn.contentType ?: "application/json; charset=utf-8")
                val stream = if (status in 200..399) conn.inputStream else conn.errorStream
                val respBody = stream?.bufferedReader(Charsets.UTF_8)?.use { it.readText() } ?: ""

                post { onShieldEvent("CORS Bridge [$upperMethod $status]: ${URL(clean).host}") }

                JSONObject().apply {
                    put("status", status)
                    put("contentType", contentType)
                    put("body", respBody)
                }.toString()
            } catch (t: Throwable) {
                JSONObject().put("error", t.message ?: "Request failed").toString()
            }
        }
    }

    init {
        setBackgroundColor(MantaTheme.BG_OBSIDIAN)
        setLayerType(View.LAYER_TYPE_HARDWARE, null)
        isFocusable = true
        isFocusableInTouchMode = true

        // Enable in-memory session cookies (including third-party iframe cookies) so Cloudflare Turnstile
        // (`challenges.cloudflare.com`), hCaptcha, and reCAPTCHA pass on the first click.
        // ZeroDiskJanitor wipes all cookies when the tab or app closes.
        try {
            CookieManager.getInstance().apply {
                setAcceptCookie(true)
                setAcceptThirdPartyCookies(this@EphemeralWebSandbox, true)
            }
        } catch (_: Throwable) {
        }

        val alignedUserAgent = buildCleanChromeUserAgent(context)

        settings.apply {
            javaScriptEnabled = true
            domStorageEnabled = true
            databaseEnabled = true
            cacheMode = WebSettings.LOAD_NO_CACHE

            useWideViewPort = true
            loadWithOverviewMode = true
            loadsImagesAutomatically = true
            blockNetworkImage = false

            setSupportZoom(true)
            builtInZoomControls = true
            displayZoomControls = false
            javaScriptCanOpenWindowsAutomatically = true
            setSupportMultipleWindows(false)
            mediaPlaybackRequiresUserGesture = false
            allowFileAccess = false
            allowContentAccess = false
            mixedContentMode = WebSettings.MIXED_CONTENT_ALWAYS_ALLOW
            userAgentString = alignedUserAgent
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

                // 1. NEVER intercept or alter Cloudflare Turnstile, hCaptcha, or reCAPTCHA challenge traffic!
                if (MantaNative.isChallengeUrl(rawUrl)) {
                    return super.shouldInterceptRequest(view, request)
                }

                // 2. Sub-millisecond Rust Shield check
                val verdict = MantaNative.sanitizeAndCheckUrl(rawUrl)
                if (verdict == "BLOCKED") {
                    val host = request.url?.host ?: rawUrl
                    post { onShieldEvent("Rust Shield Blocked: $host") }
                    return WebResourceResponse(
                        "text/plain",
                        "UTF-8",
                        204,
                        "No Content",
                        corsPermissiveHeaders(request),
                        ByteArrayInputStream(ByteArray(0))
                    )
                }

                // 3. Developer CORS Mode: Intercept OPTIONS preflights & Cross-Origin GET API calls
                if (isCorsDevEnabled()) {
                    val reqMethod = request.method?.uppercase() ?: "GET"
                    if (reqMethod == "OPTIONS") {
                        val host = request.url?.host ?: rawUrl
                        post { onShieldEvent("CORS Preflight 204: $host") }
                        return WebResourceResponse(
                            "text/plain",
                            "UTF-8",
                            204,
                            "No Content",
                            corsPermissiveHeaders(request),
                            ByteArrayInputStream(ByteArray(0))
                        )
                    }

                    val originHeader = request.requestHeaders["Origin"] ?: request.requestHeaders["origin"]
                    val targetHost = request.url?.host ?: ""
                    val isCrossOriginSubresource = !request.isForMainFrame &&
                        !originHeader.isNullOrBlank() &&
                        originHeader != "null" &&
                        !originHeader.contains(targetHost, ignoreCase = true)

                    val secFetchMode = request.requestHeaders["Sec-Fetch-Mode"] ?: ""
                    if (reqMethod == "GET" && isCrossOriginSubresource && secFetchMode.equals("cors", ignoreCase = true)) {
                        val proxied = proxyCrossOriginGet(verdict, request)
                        if (proxied != null) return proxied
                    }
                }

                return super.shouldInterceptRequest(view, request)
            }

            override fun onPageStarted(view: WebView, url: String?, favicon: Bitmap?) {
                super.onPageStarted(view, url, favicon)
                if (url != null && !MantaNative.isChallengeUrl(url)) {
                    MantaNative.prewarmDns(url)
                    val runtimeJs = MantaNative.getRuntimeScript(isCorsDevEnabled())
                    if (runtimeJs.isNotEmpty()) {
                        view.evaluateJavascript(runtimeJs, null)
                    }
                }
                if (url != null) {
                    post { onPageMetaUpdate(tabId, view.title ?: url, url) }
                }
            }

            override fun onPageFinished(view: WebView, url: String?) {
                super.onPageFinished(view, url)
                if (url != null && !MantaNative.isChallengeUrl(url)) {
                    val runtimeJs = MantaNative.getRuntimeScript(isCorsDevEnabled())
                    if (runtimeJs.isNotEmpty()) {
                        view.evaluateJavascript(runtimeJs, null)
                    }
                }
                post {
                    onProgressUpdate(tabId, 100)
                    onPageMetaUpdate(tabId, view.title ?: "Manta", url ?: "")
                }
            }
        }
    }

    private fun proxyCrossOriginGet(
        cleanUrl: String,
        request: WebResourceRequest
    ): WebResourceResponse? {
        return try {
            val conn = (URL(cleanUrl).openConnection() as HttpURLConnection).apply {
                requestMethod = "GET"
                connectTimeout = 7000
                readTimeout = 8000
                useCaches = false
                instanceFollowRedirects = true
                for ((k, v) in request.requestHeaders) {
                    if (!k.equals("Origin", ignoreCase = true) && !k.equals("Host", ignoreCase = true)) {
                        setRequestProperty(k, v)
                    }
                }
            }
            val status = conn.responseCode
            val rawCt = conn.contentType ?: "application/json; charset=utf-8"
            val mimeType = rawCt.split(';')[0].trim().ifEmpty { "application/json" }
            val stream = if (status in 200..399) conn.inputStream else conn.errorStream
            val bytes = stream?.readBytes() ?: ByteArray(0)
            val host = URL(cleanUrl).host
            post { onShieldEvent("CORS GET Unlocked: $host ($status)") }

            WebResourceResponse(
                mimeType,
                "UTF-8",
                status.coerceIn(200, 599),
                "OK",
                corsPermissiveHeaders(request),
                ByteArrayInputStream(bytes)
            )
        } catch (_: Throwable) {
            null
        }
    }

    private fun corsPermissiveHeaders(request: WebResourceRequest): Map<String, String> {
        val origin = request.requestHeaders["Origin"]
            ?: request.requestHeaders["origin"]
            ?: "*"
        val reqHeaders = request.requestHeaders["Access-Control-Request-Headers"]
            ?: request.requestHeaders["access-control-request-headers"]
            ?: "Content-Type, Authorization, X-Requested-With, Accept, Origin, Cache-Control"

        return mapOf(
            "Access-Control-Allow-Origin" to origin,
            "Access-Control-Allow-Credentials" to "true",
            "Access-Control-Allow-Methods" to "GET, POST, PUT, PATCH, DELETE, OPTIONS, HEAD",
            "Access-Control-Allow-Headers" to reqHeaders,
            "Access-Control-Expose-Headers" to "*",
            "Access-Control-Max-Age" to "86400"
        )
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
         * Reads the device's real System WebView User-Agent so the `Chrome/XXX.0.0.0` version
         * matches the TLS/HTTP2 & `Sec-CH-UA` fingerprint (mandatory for Cloudflare Turnstile),
         * while stripping the `; wv` WebView flag and specific phone hardware model.
         */
        fun buildCleanChromeUserAgent(context: Context): String {
            return try {
                val raw = WebSettings.getDefaultUserAgent(context)
                val chromeMatch = Regex("Chrome/([0-9.]+)").find(raw)?.groupValues?.getOrNull(1)
                    ?: "131.0.0.0"
                "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/$chromeMatch Mobile Safari/537.36"
            } catch (_: Throwable) {
                "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36"
            }
        }
    }
}
