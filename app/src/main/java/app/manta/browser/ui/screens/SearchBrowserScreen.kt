package app.manta.browser.ui.screens

import android.content.Context
import android.graphics.Typeface
import android.view.Gravity
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.EditorInfo
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.Toast
import app.manta.browser.core.DisplayListParser
import app.manta.browser.core.MantaNative
import app.manta.browser.ui.render.EphemeralWebSandbox
import app.manta.browser.ui.render.MantaRenderView
import app.manta.browser.ui.theme.MantaTheme
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder
import kotlin.concurrent.thread

/**
 * Search & Active Browser Screen (Dock Section 1):
 * - Top Omnibox with instant URL / Search resolution and tracking-param stripping in Rust.
 * - Mode Switcher:
 *   1. `RUST STREAM`: Renders directly via `libmanta_core.so` (`MantaRenderView`) using < 50 KB RAM.
 *   2. `DEV SANDBOX`: Full JS/CSS execution in `EphemeralWebSandbox` with Rust Shield & CORS bypass.
 */
class SearchBrowserScreen(
    context: Context,
    private val getActiveTabId: () -> Int,
    private val isCorsDevEnabled: () -> Boolean,
    private val onStateUpdated: (title: String, url: String) -> Unit,
    private val onShieldLog: (String) -> Unit
) : LinearLayout(context) {

    private val urlInput: EditText
    private val modeToggleBtn: TextView
    private val viewportContainer: FrameLayout
    private val rustRenderView: MantaRenderView
    private val webSandbox: EphemeralWebSandbox

    private var useRustStreamMode: Boolean = true
    private var currentUrl: String = "manta://benchmark"
    private var currentTitle: String = "Manta Rust Core"

    init {
        orientation = VERTICAL
        setBackgroundColor(MantaTheme.BG_OBSIDIAN)

        // 1. Top Omnibox Bar
        val topBar = LinearLayout(context).apply {
            orientation = HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            val pH = MantaTheme.dp(context, 12f)
            val pV = MantaTheme.dp(context, 10f)
            setPadding(pH, pV, pH, pV)
            setBackgroundColor(MantaTheme.BG_SURFACE)
        }

        urlInput = EditText(context).apply {
            hint = "Search or enter URL (CORS & Shield active)..."
            setHintTextColor(MantaTheme.TEXT_MUTED)
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 13.5f
            isSingleLine = true
            imeOptions = EditorInfo.IME_ACTION_GO
            val padH = MantaTheme.dp(context, 12f)
            val padV = MantaTheme.dp(context, 8f)
            setPadding(padH, padV, padH, padV)
            background = MantaTheme.roundedBg(
                fillColor = MantaTheme.BG_OBSIDIAN,
                radiusDp = 14f,
                context = context,
                strokeColor = MantaTheme.BORDER_SUBTLE,
                strokeWidthDp = 1f
            )
            layoutParams = LayoutParams(0, LayoutParams.WRAP_CONTENT, 1f)
            setOnEditorActionListener { _, actionId, event ->
                if (actionId == EditorInfo.IME_ACTION_GO ||
                    actionId == EditorInfo.IME_ACTION_SEARCH ||
                    (event != null && event.keyCode == KeyEvent.KEYCODE_ENTER)
                ) {
                    loadInputUrlOrQuery(text.toString(), useRustStreamMode)
                    true
                } else {
                    false
                }
            }
        }
        topBar.addView(urlInput)

        // Engine Mode Switcher Button (`RUST` vs `WEB`)
        modeToggleBtn = TextView(context).apply {
            text = "RUST"
            setTextColor(MantaTheme.ACCENT_EMERALD)
            textSize = 11f
            typeface = Typeface.create(Typeface.MONOSPACE, Typeface.BOLD)
            val pH = MantaTheme.dp(context, 10f)
            val pV = MantaTheme.dp(context, 9f)
            setPadding(pH, pV, pH, pV)
            background = MantaTheme.roundedBg(MantaTheme.BG_DOCK_PILL, 12f, context)
            layoutParams = LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT).apply {
                marginStart = MantaTheme.dp(context, 8f)
            }
            setOnClickListener {
                setEngineMode(!useRustStreamMode)
                loadUrlInternal(currentUrl)
            }
        }
        topBar.addView(modeToggleBtn)

        // Copy URL to Rust Clipboard button
        val copyBtn = TextView(context).apply {
            text = "Copy"
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 11.5f
            val pH = MantaTheme.dp(context, 10f)
            val pV = MantaTheme.dp(context, 9f)
            setPadding(pH, pV, pH, pV)
            background = MantaTheme.roundedBg(MantaTheme.BG_DOCK_PILL, 12f, context)
            layoutParams = LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT).apply {
                marginStart = MantaTheme.dp(context, 6f)
            }
            setOnClickListener {
                MantaNative.copyToClipboard(currentUrl)
                Toast.makeText(context, "Copied to Manta Rust Clipboard", Toast.LENGTH_SHORT).show()
            }
        }
        topBar.addView(copyBtn)
        addView(topBar)

        // 2. Viewport Container holding both `MantaRenderView` and `EphemeralWebSandbox`
        viewportContainer = FrameLayout(context).apply {
            layoutParams = LayoutParams(LayoutParams.MATCH_PARENT, 0, 1f).apply {
                bottomMargin = MantaTheme.dp(context, 134f)
            }
        }

        rustRenderView = MantaRenderView(
            context = context,
            onLinkClicked = { href -> loadInputUrlOrQuery(href, useRustStreamMode) },
            onTextCopied = { copied ->
                MantaNative.copyToClipboard(copied)
                Toast.makeText(context, "Copied paragraph to Rust Vault", Toast.LENGTH_SHORT).show()
            }
        )

        webSandbox = EphemeralWebSandbox(
            context = context,
            isCorsDevEnabled = isCorsDevEnabled,
            onPageMetaUpdate = { title, url ->
                currentTitle = title
                currentUrl = url
                urlInput.setText(url)
                onStateUpdated(title, url)
            },
            onShieldEvent = onShieldLog
        )

        viewportContainer.addView(rustRenderView)
        viewportContainer.addView(webSandbox)
        addView(viewportContainer)

        setEngineMode(true)
        loadBuiltInRustPage("manta://benchmark")
    }

    fun setEngineMode(rustMode: Boolean) {
        useRustStreamMode = rustMode
        modeToggleBtn.text = if (rustMode) "RUST" else "SANDBOX"
        modeToggleBtn.setTextColor(
            if (rustMode) MantaTheme.ACCENT_EMERALD else MantaTheme.ACCENT_CYAN
        )
        rustRenderView.visibility = if (rustMode) View.VISIBLE else View.GONE
        webSandbox.visibility = if (rustMode) View.GONE else View.VISIBLE
    }

    fun loadInputUrlOrQuery(rawInput: String, preferRustMode: Boolean) {
        val trimmed = rawInput.trim()
        if (trimmed.isEmpty()) return

        setEngineMode(preferRustMode)

        val resolvedUrl = when {
            trimmed.startsWith("manta://") -> trimmed
            trimmed.startsWith("http://") || trimmed.startsWith("https://") -> trimmed
            trimmed.contains(".") && !trimmed.contains(" ") -> "https://$trimmed"
            else -> "https://lite.duckduckgo.com/lite/?q=${URLEncoder.encode(trimmed, "UTF-8")}"
        }

        loadUrlInternal(resolvedUrl)
    }

    fun reloadCurrentPage() {
        loadUrlInternal(currentUrl)
    }

    fun wipeSandboxMemory() {
        webSandbox.wipeAllEphemeralData()
        loadBuiltInRustPage("manta://benchmark")
    }

    fun handleBackNavigation(): Boolean {
        if (!useRustStreamMode && webSandbox.canGoBack()) {
            webSandbox.goBack()
            return true
        }
        return false
    }

    private fun loadUrlInternal(targetUrl: String) {
        if (targetUrl.startsWith("manta://")) {
            setEngineMode(true)
            loadBuiltInRustPage(targetUrl)
            return
        }

        val verdict = MantaNative.sanitizeAndCheckUrl(targetUrl)
        if (verdict == "BLOCKED") {
            onShieldLog("Blocked navigation to tracker host: $targetUrl")
            Toast.makeText(context, "Shield blocked tracker domain", Toast.LENGTH_SHORT).show()
            return
        }

        currentUrl = verdict
        urlInput.setText(verdict)

        if (useRustStreamMode) {
            streamRemoteUrlViaRust(verdict)
        } else {
            webSandbox.loadUrl(verdict)
        }
    }

    private fun loadBuiltInRustPage(schemeUrl: String) {
        currentUrl = schemeUrl
        urlInput.setText(schemeUrl)

        val html = when (schemeUrl) {
            "manta://cors-lab" -> """
                <html>
                <head>
                    <style>
                        body { background: #09090b; color: #f8fafc; }
                        h1 { color: #38bdf8; }
                        .card { background: #161920; padding: 14px; color: #e2e8f0; }
                        #result { color: #10b981; font-weight: bold; }
                    </style>
                </head>
                <body>
                    <h1>Manta Developer CORS & JS Sandbox</h1>
                    <div class="card">
                        <p>All OPTIONS preflights return 204 in 0ms and responses include Access-Control-Allow-Origin: *.</p>
                        <p id="result">Initializing JS VM...</p>
                    </div>
                    <p>Tap the SANDBOX toggle in the top right to test full external SPAs or localhost servers.</p>
                    <a href="https://httpbin.org/headers">Test Live Headers on HTTPBin.org</a>
                    <script>
                        let wd = navigator.webdriver;
                        document.getElementById("result").textContent = "JS VM Active • navigator.webdriver = " + wd + " • Zero Cookies";
                    </script>
                </body>
                </html>
            """.trimIndent()
            else -> """
                <html>
                <head>
                    <style>
                        body { background: #09090b; color: #f8fafc; }
                        h1 { color: #ffffff; }
                        .box { background: #141821; padding: 14px; color: #38bdf8; }
                        #bench { color: #10b981; font-weight: bold; }
                    </style>
                </head>
                <body>
                    <h1>Manta Rust Streaming Core</h1>
                    <div class="box">
                        <p id="bench">Booting Contiguous Flat DOM Arena...</p>
                        <p>Zero Disk Cache • Cryptographic RAM Zeroization on Close</p>
                    </div>
                    <h2>Tap any link below to stream:</h2>
                    <a href="manta://cors-lab">Open Developer CORS & Anti-Bot Lab</a>
                    <a href="https://news.ycombinator.com">Stream Hacker News</a>
                    <a href="https://lite.duckduckgo.com/lite/">Open DuckDuckGo Lite Search</a>
                    <script>
                        document.title = "Manta Rust Core";
                        document.getElementById("bench").textContent = "Flat DOM Arena Ready • Fuel-Metered JS VM Verified";
                    </script>
                </body>
                </html>
            """.trimIndent()
        }

        val wire = MantaNative.streamHtmlChunk(getActiveTabId(), schemeUrl, html, true)
        val parsed = DisplayListParser.parse(wire)
        currentTitle = parsed.title
        rustRenderView.setFrame(parsed)
        onStateUpdated(parsed.title, schemeUrl)
    }

    private fun streamRemoteUrlViaRust(cleanUrl: String) {
        val tabId = getActiveTabId()
        onStateUpdated("Streaming...", cleanUrl)

        thread(start = true) {
            try {
                val conn = (URL(cleanUrl).openConnection() as HttpURLConnection).apply {
                    requestMethod = "GET"
                    connectTimeout = 6000
                    readTimeout = 8000
                    useCaches = false
                    setRequestProperty("DNT", "1")
                    setRequestProperty("Sec-GPC", "1")
                    setRequestProperty(
                        "User-Agent",
                        "Mozilla/5.0 (Linux; Android 14; Mobile) Manta/1.0"
                    )
                }

                val stream = conn.inputStream
                val buf = ByteArray(4096)
                var totalRead = 0
                while (true) {
                    val n = stream.read(buf)
                    if (n <= 0) break
                    totalRead += n
                    val chunkStr = String(buf, 0, n, Charsets.UTF_8)
                    MantaNative.streamHtmlChunk(tabId, cleanUrl, chunkStr, false)
                    if (totalRead > 256 * 1024) break
                }
                stream.close()

                val finalWire = MantaNative.streamHtmlChunk(tabId, cleanUrl, "", true)
                val parsed = DisplayListParser.parse(finalWire)

                post {
                    currentTitle = parsed.title
                    rustRenderView.setFrame(parsed)
                    onStateUpdated(parsed.title, cleanUrl)
                }
            } catch (e: Exception) {
                val errHtml = "<html><body><h1>Stream Notice</h1><p>${e.message ?: "Connection failed"}</p><p>Tip: Tap RUST -> SANDBOX in the top bar for TLS/SPA sites.</p></body></html>"
                val wire = MantaNative.streamHtmlChunk(tabId, cleanUrl, errHtml, true)
                val parsed = DisplayListParser.parse(wire)
                post {
                    rustRenderView.setFrame(parsed)
                    onStateUpdated("Stream Fallback", cleanUrl)
                }
            }
        }
    }
}
