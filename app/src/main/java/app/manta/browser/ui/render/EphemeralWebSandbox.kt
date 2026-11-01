package app.manta.browser.ui.render

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Bitmap
import android.webkit.CookieManager
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebSettings
import android.webkit.WebStorage
import android.webkit.WebView
import android.webkit.WebViewClient
import app.manta.browser.core.MantaNative
import app.manta.browser.ui.theme.MantaTheme
import java.io.ByteArrayInputStream
import java.net.HttpURLConnection
import java.net.URL

/**
 * Rust-Shielded Ephemeral Web Sandbox:
 * - Zero disk cache (`LOAD_NO_CACHE`), zero cookies, zero DOM storage persistence.
 * - Routes every request through `MantaNative.sanitizeAndCheckUrl` in Rust.
 * - Synthesizes 0ms `OPTIONS` CORS preflights and injects permissive CORS headers
 *   (`Access-Control-Allow-Origin: *`, etc.) so developers can test any site or API.
 * - Injects anti-JS-detection script (`navigator.webdriver = false`).
 */
@SuppressLint("SetJavaScriptEnabled")
class EphemeralWebSandbox(
    context: Context,
    private val isCorsDevEnabled: () -> Boolean,
    private val onPageMetaUpdate: (title: String, url: String) -> Unit,
    private val onShieldEvent: (String) -> Unit
) : WebView(context) {

    init {
        setBackgroundColor(MantaTheme.BG_OBSIDIAN)

        // Strictly disable all cookies & third-party persistence
        CookieManager.getInstance().apply {
            setAcceptCookie(false)
            setAcceptThirdPartyCookies(this@EphemeralWebSandbox, false)
            removeAllCookies(null)
        }

        settings.apply {
            javaScriptEnabled = true
            domStorageEnabled = false
            databaseEnabled = false
            cacheMode = WebSettings.LOAD_NO_CACHE
            setSupportZoom(true)
            builtInZoomControls = true
            displayZoomControls = false
            mixedContentMode = WebSettings.MIXED_CONTENT_ALWAYS_ALLOW
            userAgentString =
                "Mozilla/5.0 (Linux; Android 14; Mobile) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36"
        }

        webViewClient = object : WebViewClient() {
            override fun shouldInterceptRequest(
                view: WebView,
                request: WebResourceRequest
            ): WebResourceResponse? {
                val rawUrl = request.url.toString()
                val verdict = MantaNative.sanitizeAndCheckUrl(rawUrl)

                // 1. Blocked tracker domain in Rust Shield
                if (verdict == "BLOCKED") {
                    onShieldEvent("Blocked Tracker: ${request.url.host}")
                    return WebResourceResponse(
                        "text/plain",
                        "UTF-8",
                        204,
                        "No Content",
                        mapOf("Access-Control-Allow-Origin" to "*"),
                        ByteArrayInputStream(ByteArray(0))
                    )
                }

                // 2. Developer CORS OPTIONS Preflight Synthesizer
                if (isCorsDevEnabled() && request.method.equals("OPTIONS", ignoreCase = true)) {
                    onShieldEvent("CORS Preflight Allowed: ${request.url.host}")
                    return WebResourceResponse(
                        "text/plain",
                        "UTF-8",
                        204,
                        "No Content",
                        corsPermissiveHeaders(),
                        ByteArrayInputStream(ByteArray(0))
                    )
                }

                // 3. For XHR/Fetch requests when CORS Dev Mode is ON, proxy with CORS headers
                val accept = request.requestHeaders["Accept"] ?: ""
                val isSubresourceApi = ! request.isForMainFrame &&
                    (accept.contains("application/json") || request.requestHeaders.containsKey("Origin"))

                if (isCorsDevEnabled() && isSubresourceApi && verdict.startsWith("http")) {
                    return fetchWithCorsBypass(verdict, request)
                }

                return super.shouldInterceptRequest(view, request)
            }

            override fun onPageStarted(view: WebView, url: String?, favicon: Bitmap?) {
                super.onPageStarted(view, url, favicon)
                // Inject Anti-Bot / Anti-JS-Detection Shield before page scripts run
                val antiDetectionJs = """
                    (function() {
                        Object.defineProperty(navigator, 'webdriver', { get: () => false });
                        Object.defineProperty(navigator, 'GlobalPrivacyControl', { get: () => true });
                        window.chrome = { runtime: {} };
                    })();
                """.trimIndent()
                view.evaluateJavascript(antiDetectionJs, null)
                if (url != null) {
                    onPageMetaUpdate(view.title ?: "Loading...", url)
                }
            }

            override fun onPageFinished(view: WebView, url: String?) {
                super.onPageFinished(view, url)
                onPageMetaUpdate(view.title ?: "Manta Sandbox", url ?: "")
            }
        }
    }

    private fun fetchWithCorsBypass(
        cleanUrl: String,
        request: WebResourceRequest
    ): WebResourceResponse? {
        return try {
            val conn = (URL(cleanUrl).openConnection() as HttpURLConnection).apply {
                requestMethod = request.method
                connectTimeout = 6000
                readTimeout = 8000
                useCaches = false
                setRequestProperty("DNT", "1")
                setRequestProperty("Sec-GPC", "1")
            }
            val status = conn.responseCode
            val contentType = (conn.contentType ?: "application/json").split(';')[0].trim()
            val stream = if (status in 200..399) conn.inputStream else conn.errorStream
            val bytes = stream?.readBytes() ?: ByteArray(0)
            onShieldEvent("CORS Header Injected: ${URL(cleanUrl).host}")

            WebResourceResponse(
                contentType,
                "UTF-8",
                status.coerceIn(200, 599),
                "OK",
                corsPermissiveHeaders(),
                ByteArrayInputStream(bytes)
            )
        } catch (_: Exception) {
            null
        }
    }

    private fun corsPermissiveHeaders(): Map<String, String> = mapOf(
        "Access-Control-Allow-Origin" to "*",
        "Access-Control-Allow-Methods" to "GET, POST, PUT, PATCH, DELETE, OPTIONS, HEAD",
        "Access-Control-Allow-Headers" to "*",
        "Access-Control-Expose-Headers" to "*",
        "Access-Control-Allow-Credentials" to "true"
    )

    fun wipeAllEphemeralData() {
        stopLoading()
        loadUrl("about:blank")
        clearHistory()
        clearFormData()
        clearCache(true)
        CookieManager.getInstance().removeAllCookies(null)
        WebStorage.getInstance().deleteAllData()
    }
}
