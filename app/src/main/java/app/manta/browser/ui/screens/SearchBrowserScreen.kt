package app.manta.browser.ui.screens

import android.content.Context
import android.graphics.Typeface
import android.text.TextUtils
import android.view.Gravity
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.HorizontalScrollView
import android.widget.LinearLayout
import android.widget.ProgressBar
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
 * Multi-Tab Search & Active Browser Screen (Dock Section 1):
 * - Preserves multiple open tabs in memory with instant switching and automatic background hibernation.
 * - Top Tab Strip allows 1-tap switching, creating new tabs (`+`), and closing/zeroizing tabs (`✕`).
 * - Full Cloudflare Turnstile, CAPTCHA, Cookie-Banner Auto-Dismiss, and Developer CORS support.
 */
class SearchBrowserScreen(
    context: Context,
    private val openTabs: MutableList<EphemeralTabInfo>,
    private val getActiveTabId: () -> Int,
    private val setActiveTabId: (Int) -> Unit,
    private val isCorsDevEnabled: () -> Boolean,
    private val onStateUpdated: (title: String, url: String) -> Unit,
    private val onShieldLog: (String) -> Unit
) : LinearLayout(context) {

    private val tabStripRow: LinearLayout
    private val urlInput: EditText
    private val modeToggleBtn: TextView
    private val progressBar: ProgressBar
    private val viewportContainer: FrameLayout
    private val rustRenderView: MantaRenderView

    // Multi-Tab Sandbox Pool: preserves live tabs while hibernating background tabs
    private val tabSandboxes = HashMap<Int, EphemeralWebSandbox>()
    private val maxLiveSandboxesInRam = 4

    private var useRustReaderMode: Boolean = false
    private var currentUrl: String = "https://duckduckgo.com"

    init {
        orientation = VERTICAL
        setBackgroundColor(MantaTheme.BG_OBSIDIAN)

        // 1. Compact Multi-Tab Switcher Bar at the very top
        val tabBarContainer = LinearLayout(context).apply {
            orientation = HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            val pH = MantaTheme.dp(context, 10f)
            val pTop = MantaTheme.dp(context, 8f)
            setPadding(pH, pTop, pH, MantaTheme.dp(context, 4f))
            setBackgroundColor(MantaTheme.BG_SURFACE)
        }

        val tabStripScroll = HorizontalScrollView(context).apply {
            isHorizontalScrollBarEnabled = false
            layoutParams = LayoutParams(0, LayoutParams.WRAP_CONTENT, 1f)
        }

        tabStripRow = LinearLayout(context).apply {
            orientation = HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
        }
        tabStripScroll.addView(tabStripRow)
        tabBarContainer.addView(tabStripScroll)

        val addTabBtn = TextView(context).apply {
            text = "+ Tab"
            setTextColor(MantaTheme.BG_OBSIDIAN)
            textSize = 11.5f
            typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            val pH = MantaTheme.dp(context, 11f)
            val pV = MantaTheme.dp(context, 6f)
            setPadding(pH, pV, pH, pV)
            background = MantaTheme.roundedBg(MantaTheme.TEXT_PRIMARY, 12f, context)
            layoutParams = LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT).apply {
                marginStart = MantaTheme.dp(context, 8f)
            }
            setOnClickListener {
                createNewTabAndSwitch("https://duckduckgo.com")
            }
        }
        tabBarContainer.addView(addTabBtn)
        addView(tabBarContainer)

        // 2. Omnibox & Navigation Controls Bar
        val topBar = LinearLayout(context).apply {
            orientation = HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            val pH = MantaTheme.dp(context, 10f)
            val pV = MantaTheme.dp(context, 7f)
            setPadding(pH, pV, pH, pV)
            setBackgroundColor(MantaTheme.BG_SURFACE)
        }

        val backBtn = TextView(context).apply {
            text = "‹"
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 20f
            gravity = Gravity.CENTER
            val s = MantaTheme.dp(context, 34f)
            layoutParams = LayoutParams(s, s).apply {
                marginEnd = MantaTheme.dp(context, 6f)
            }
            background = MantaTheme.roundedBg(MantaTheme.BG_DOCK_PILL, 10f, context)
            setOnClickListener {
                val activeSandbox = tabSandboxes[getActiveTabId()]
                if (activeSandbox?.canGoBack() == true) {
                    activeSandbox.goBack()
                }
            }
        }
        topBar.addView(backBtn)

        urlInput = EditText(context).apply {
            hint = "Search or enter URL (CORS & Cookie-Zapper ON)..."
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
                    hideKeyboard()
                    val raw = text.toString()
                    loadInputUrlOrQuery(raw, raw.startsWith("manta://"))
                    true
                } else {
                    false
                }
            }
        }
        topBar.addView(urlInput)

        modeToggleBtn = TextView(context).apply {
            text = "FULL"
            setTextColor(MantaTheme.ACCENT_EMERALD)
            textSize = 10.5f
            typeface = Typeface.create(Typeface.MONOSPACE, Typeface.BOLD)
            val pH = MantaTheme.dp(context, 10f)
            val pV = MantaTheme.dp(context, 9f)
            setPadding(pH, pV, pH, pV)
            background = MantaTheme.roundedBg(MantaTheme.BG_DOCK_PILL, 10f, context)
            layoutParams = LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT).apply {
                marginStart = MantaTheme.dp(context, 6f)
            }
            setOnClickListener {
                setReaderMode(!useRustReaderMode)
                loadUrlInternal(currentUrl, forceReload = true)
            }
        }
        topBar.addView(modeToggleBtn)

        val copyBtn = TextView(context).apply {
            text = "Copy"
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 11f
            val pH = MantaTheme.dp(context, 9f)
            val pV = MantaTheme.dp(context, 9f)
            setPadding(pH, pV, pH, pV)
            background = MantaTheme.roundedBg(MantaTheme.BG_DOCK_PILL, 10f, context)
            layoutParams = LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT).apply {
                marginStart = MantaTheme.dp(context, 5f)
            }
            setOnClickListener {
                MantaNative.copyToClipboard(currentUrl)
                Toast.makeText(context, "Copied URL to Rust Clipboard", Toast.LENGTH_SHORT).show()
            }
        }
        topBar.addView(copyBtn)
        addView(topBar)

        // 3. Loading Progress Bar
        progressBar = ProgressBar(context, null, android.R.attr.progressBarStyleHorizontal).apply {
            max = 100
            progress = 0
            visibility = View.GONE
            layoutParams = LayoutParams(LayoutParams.MATCH_PARENT, MantaTheme.dp(context, 2.5f))
        }
        addView(progressBar)

        // 4. Viewport Container holding preserved Multi-Tab Sandboxes + Rust Reader View
        viewportContainer = FrameLayout(context).apply {
            layoutParams = LayoutParams(LayoutParams.MATCH_PARENT, 0, 1f).apply {
                bottomMargin = MantaTheme.dp(context, 134f)
            }
        }

        rustRenderView = MantaRenderView(
            context = context,
            onLinkClicked = { href -> loadInputUrlOrQuery(href, href.startsWith("manta://")) },
            onTextCopied = { copied ->
                MantaNative.copyToClipboard(copied)
                Toast.makeText(context, "Copied paragraph to Rust Vault", Toast.LENGTH_SHORT).show()
            }
        )

        viewportContainer.addView(rustRenderView)
        addView(viewportContainer)
    }

    fun bootInitialPage() {
        renderTabStrip()
        loadBuiltInRustPage("manta://benchmark")
    }

    /**
     * Renders the interactive horizontal tab strip at the top of the browser.
     */
    fun renderTabStrip() {
        tabStripRow.removeAllViews()
        val activeId = getActiveTabId()

        for (tab in openTabs) {
            val isSelected = tab.id == activeId
            val pill = LinearLayout(context).apply {
                orientation = HORIZONTAL
                gravity = Gravity.CENTER_VERTICAL
                val pH = MantaTheme.dp(context, 10f)
                val pV = MantaTheme.dp(context, 5f)
                setPadding(pH, pV, pH, pV)
                background = MantaTheme.roundedBg(
                    fillColor = if (isSelected) MantaTheme.BG_DOCK_PILL else MantaTheme.BG_OBSIDIAN,
                    radiusDp = 12f,
                    context = context,
                    strokeColor = if (isSelected) MantaTheme.ACCENT_CYAN else MantaTheme.BORDER_SUBTLE,
                    strokeWidthDp = 1f
                )
                layoutParams = LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT).apply {
                    marginEnd = MantaTheme.dp(context, 6f)
                }
                setOnClickListener {
                    switchToExistingTab(tab.id)
                }
            }

            val titleTv = TextView(context).apply {
                text = tab.title.ifBlank { "Tab ${tab.id}" }
                setTextColor(if (isSelected) MantaTheme.TEXT_PRIMARY else MantaTheme.TEXT_SECONDARY)
                textSize = 11.5f
                maxWidth = MantaTheme.dp(context, 120f)
                maxLines = 1
                ellipsize = TextUtils.TruncateAt.END
                typeface = Typeface.create(
                    Typeface.SANS_SERIF,
                    if (isSelected) Typeface.BOLD else Typeface.NORMAL
                )
            }
            pill.addView(titleTv)

            val closeTv = TextView(context).apply {
                text = " ✕"
                setTextColor(MantaTheme.TEXT_MUTED)
                textSize = 11f
                setPadding(MantaTheme.dp(context, 4f), 0, 0, 0)
                setOnClickListener {
                    closeTabById(tab.id)
                }
            }
            pill.addView(closeTv)
            tabStripRow.addView(pill)
        }
    }

    /**
     * Switches between open tabs while preserving the live page state in memory
     * and hibernating background tabs in Rust's compressed vault.
     */
    fun switchToExistingTab(targetTabId: Int) {
        val previousId = getActiveTabId()
        if (previousId != targetTabId) {
            tabSandboxes[previousId]?.let { prevView ->
                prevView.hibernateInBackground()
                prevView.visibility = View.GONE
            }
        }

        setActiveTabId(targetTabId)
        MantaNative.switchTab(targetTabId)

        val tabInfo = openTabs.find { it.id == targetTabId }
        val targetUrl = tabInfo?.url ?: "https://duckduckgo.com"
        currentUrl = targetUrl
        urlInput.setText(targetUrl)

        val existingSandbox = tabSandboxes[targetTabId]
        if (existingSandbox != null && !targetUrl.startsWith("manta://")) {
            setReaderMode(false)
            existingSandbox.visibility = View.VISIBLE
            existingSandbox.wakeFromHibernation()
            onStateUpdated(tabInfo?.title ?: "Manta", targetUrl)
        } else {
            loadInputUrlOrQuery(targetUrl, targetUrl.startsWith("manta://"))
        }
        renderTabStrip()
    }

    fun createNewTabAndSwitch(initialUrl: String) {
        val prevId = getActiveTabId()
        tabSandboxes[prevId]?.let {
            it.hibernateInBackground()
            it.visibility = View.GONE
        }

        val newId = MantaNative.createNewTab()
        openTabs.add(EphemeralTabInfo(newId, "New Tab", initialUrl))
        setActiveTabId(newId)
        MantaNative.switchTab(newId)
        loadInputUrlOrQuery(initialUrl, initialUrl.startsWith("manta://"))
        renderTabStrip()
    }

    fun closeTabById(tabId: Int) {
        tabSandboxes.remove(tabId)?.let { sandbox ->
            sandbox.wipeAllEphemeralData()
            viewportContainer.removeView(sandbox)
            sandbox.destroy()
        }
        MantaNative.closeTabAndZeroize(tabId)
        openTabs.removeAll { it.id == tabId }

        if (openTabs.isEmpty()) {
            val freshId = MantaNative.createNewTab()
            openTabs.add(EphemeralTabInfo(freshId, "Manta Browser", "https://duckduckgo.com"))
            setActiveTabId(freshId)
            loadInputUrlOrQuery("https://duckduckgo.com", false)
        } else if (getActiveTabId() == tabId) {
            val nextTab = openTabs.last()
            switchToExistingTab(nextTab.id)
        }
        renderTabStrip()
    }

    private fun getOrCreateSandboxForTab(tabId: Int): EphemeralWebSandbox? {
        tabSandboxes[tabId]?.let { return it }

        // Enforce strict RAM cap: if more than `maxLiveSandboxesInRam` are active, hibernate & evict oldest background view
        if (tabSandboxes.size >= maxLiveSandboxesInRam) {
            val oldestId = tabSandboxes.keys.firstOrNull { it != tabId }
            if (oldestId != null) {
                tabSandboxes.remove(oldestId)?.let { oldView ->
                    oldView.hibernateInBackground()
                    viewportContainer.removeView(oldView)
                    oldView.destroy()
                }
            }
        }

        return try {
            val created = EphemeralWebSandbox(
                context = context,
                tabId = tabId,
                isCorsDevEnabled = isCorsDevEnabled,
                onPageMetaUpdate = { updatedTabId, title, url ->
                    openTabs.find { it.id == updatedTabId }?.let {
                        it.title = title
                        if (url.isNotBlank()) it.url = url
                    }
                    if (updatedTabId == getActiveTabId()) {
                        if (url.isNotBlank()) {
                            currentUrl = url
                            if (!urlInput.hasFocus()) {
                                urlInput.setText(url)
                            }
                        }
                        onStateUpdated(title, currentUrl)
                    }
                    renderTabStrip()
                },
                onProgressUpdate = { updatedTabId, pct ->
                    if (updatedTabId == getActiveTabId()) {
                        if (pct in 1..99) {
                            progressBar.visibility = View.VISIBLE
                            progressBar.progress = pct
                        } else {
                            progressBar.visibility = View.GONE
                        }
                    }
                },
                onShieldEvent = onShieldLog
            )
            viewportContainer.addView(
                created,
                FrameLayout.LayoutParams(
                    FrameLayout.LayoutParams.MATCH_PARENT,
                    FrameLayout.LayoutParams.MATCH_PARENT
                )
            )
            tabSandboxes[tabId] = created
            created
        } catch (_: Throwable) {
            null
        }
    }

    fun setReaderMode(readerMode: Boolean) {
        useRustReaderMode = readerMode
        modeToggleBtn.text = if (readerMode) "READER" else "FULL"
        modeToggleBtn.setTextColor(
            if (readerMode) MantaTheme.ACCENT_CYAN else MantaTheme.ACCENT_EMERALD
        )
        rustRenderView.visibility = if (readerMode) View.VISIBLE else View.GONE
        val activeId = getActiveTabId()
        tabSandboxes.forEach { (id, view) ->
            view.visibility = if (!readerMode && id == activeId) View.VISIBLE else View.GONE
        }
    }

    fun loadInputUrlOrQuery(rawInput: String, preferRustReader: Boolean) {
        val trimmed = rawInput.trim()
        if (trimmed.isEmpty()) return

        val resolvedUrl = when {
            trimmed.startsWith("manta://") -> trimmed
            trimmed.startsWith("http://") || trimmed.startsWith("https://") -> trimmed
            trimmed.contains(".") && !trimmed.contains(" ") -> "https://$trimmed"
            else -> "https://duckduckgo.com/?q=${URLEncoder.encode(trimmed, "UTF-8")}"
        }

        MantaNative.prewarmDns(resolvedUrl)
        setReaderMode(preferRustReader || resolvedUrl.startsWith("manta://"))
        loadUrlInternal(resolvedUrl, forceReload = false)
    }

    fun reloadCurrentPage() {
        loadUrlInternal(currentUrl, forceReload = true)
    }

    fun wipeAllTabsAndSandboxes() {
        for ((_, sandbox) in tabSandboxes) {
            sandbox.wipeAllEphemeralData()
            viewportContainer.removeView(sandbox)
            sandbox.destroy()
        }
        tabSandboxes.clear()
        loadBuiltInRustPage("manta://benchmark")
    }

    fun handleBackNavigation(): Boolean {
        val activeSandbox = tabSandboxes[getActiveTabId()]
        if (!useRustReaderMode && activeSandbox != null && activeSandbox.canGoBack()) {
            activeSandbox.goBack()
            return true
        }
        return false
    }

    private fun hideKeyboard() {
        try {
            val imm = context.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager
            imm?.hideSoftInputFromWindow(urlInput.windowToken, 0)
            urlInput.clearFocus()
        } catch (_: Throwable) {
        }
    }

    private fun loadUrlInternal(targetUrl: String, forceReload: Boolean) {
        val activeId = getActiveTabId()
        openTabs.find { it.id == activeId }?.url = targetUrl

        if (targetUrl == "manta://cors-lab") {
            setReaderMode(false)
            loadInteractiveCorsLabInFullEngine(activeId)
            return
        }

        if (targetUrl.startsWith("manta://")) {
            setReaderMode(true)
            loadBuiltInRustPage(targetUrl)
            return
        }

        val verdict = MantaNative.sanitizeAndCheckUrl(targetUrl)
        if (verdict == "BLOCKED") {
            onShieldLog("Blocked navigation to tracker host: $targetUrl")
            Toast.makeText(context, "Rust Shield blocked tracker domain", Toast.LENGTH_SHORT).show()
            return
        }

        currentUrl = verdict
        urlInput.setText(verdict)

        if (useRustReaderMode) {
            streamRemoteUrlViaRust(verdict)
        } else {
            tabSandboxes.forEach { (id, view) ->
                if (id != activeId) {
                    view.hibernateInBackground()
                    view.visibility = View.GONE
                }
            }
            val sandbox = getOrCreateSandboxForTab(activeId)
            if (sandbox != null) {
                sandbox.visibility = View.VISIBLE
                sandbox.wakeFromHibernation()
                rustRenderView.visibility = View.GONE
                if (forceReload || sandbox.url != verdict) {
                    sandbox.loadUrl(verdict)
                }
            } else {
                setReaderMode(true)
                streamRemoteUrlViaRust(verdict)
            }
        }
        renderTabStrip()
    }

    /**
     * Interactive Live Developer CORS & Turnstile Diagnostic Lab running inside the Full Engine
     * so the user can test real cross-origin `fetch()` and `XMLHttpRequest` calls with 1 click.
     */
    private fun loadInteractiveCorsLabInFullEngine(tabId: Int) {
        currentUrl = "manta://cors-lab"
        urlInput.setText("manta://cors-lab")
        val sandbox = getOrCreateSandboxForTab(tabId) ?: return
        sandbox.visibility = View.VISIBLE
        rustRenderView.visibility = View.GONE

        val labHtml = """
            <!DOCTYPE html>
            <html>
            <head>
                <meta name="viewport" content="width=device-width, initial-scale=1.0">
                <title>Manta CORS & Shield Lab</title>
                <style>
                    body { background:#09090b; color:#f8fafc; font-family:sans-serif; padding:16px; margin:0; }
                    .card { background:#14171f; border:1px solid #242936; border-radius:14px; padding:14px; margin-bottom:14px; }
                    h2 { margin:0 0 8px 0; font-size:17px; color:#38bdf8; }
                    p { margin:4px 0; font-size:13px; color:#94a3b8; }
                    button { background:#10b981; color:#000; border:none; border-radius:10px; padding:10px 14px; font-weight:bold; font-size:13px; margin-top:8px; margin-right:6px; }
                    pre { background:#000; color:#10b981; padding:10px; border-radius:10px; font-size:11.5px; overflow-x:auto; white-space:pre-wrap; }
                </style>
            </head>
            <body>
                <div class="card">
                    <h2>1. Live Cross-Origin CORS Tester</h2>
                    <p>Tests cross-origin fetch() & POST requests across external domains with Manta's Rust CORS Engine:</p>
                    <button onclick="runCorsGet()">Test Cross-Origin GET</button>
                    <button onclick="runCorsPost()">Test Cross-Origin POST</button>
                    <pre id="cors-out">Tap a button above to test cross-origin API calls...</pre>
                </div>
                <div class="card">
                    <h2>2. Cloudflare Turnstile & Anti-Bot Integrity</h2>
                    <pre id="bot-out">Checking browser fingerprint integrity...</pre>
                </div>
                <script>
                    document.getElementById('bot-out').textContent =
                        'navigator.webdriver: ' + navigator.webdriver + '\n' +
                        'User-Agent: ' + navigator.userAgent + '\n' +
                        'Cookies Enabled (Session RAM): ' + navigator.cookieEnabled + '\n' +
                        'Worker Cores: ' + (navigator.hardwareConcurrency || 'Auto');

                    async function runCorsGet() {
                        const out = document.getElementById('cors-out');
                        out.textContent = 'Executing cross-origin GET to https://httpbin.org/get ...';
                        try {
                            const res = await fetch('https://httpbin.org/get?manta_cors=1');
                            const txt = await res.text();
                            out.textContent = 'SUCCESS (HTTP ' + res.status + '):\n' + txt.slice(0, 400);
                        } catch (e) {
                            out.textContent = 'ERROR: ' + e;
                        }
                    }

                    async function runCorsPost() {
                        const out = document.getElementById('cors-out');
                        out.textContent = 'Executing cross-origin POST via MantaBridge ...';
                        try {
                            const res = await fetch('https://httpbin.org/post', {
                                method: 'POST',
                                headers: { 'Content-Type': 'application/json', 'X-Custom-Dev': 'Manta' },
                                body: JSON.stringify({ engine: 'Manta Rust Core', corsBypass: true })
                            });
                            const txt = await res.text();
                            out.textContent = 'POST SUCCESS (HTTP ' + res.status + '):\n' + txt.slice(0, 450);
                        } catch (e) {
                            out.textContent = 'ERROR: ' + e;
                        }
                    }
                </script>
            </body>
            </html>
        """.trimIndent()

        sandbox.loadDataWithBaseURL(
            "https://manta.local/",
            labHtml,
            "text/html",
            "UTF-8",
            null
        )
        onStateUpdated("CORS & Shield Lab", "manta://cors-lab")
        renderTabStrip()
    }

    private fun loadBuiltInRustPage(schemeUrl: String) {
        currentUrl = schemeUrl
        urlInput.setText(schemeUrl)
        val cores = MantaNative.getWorkerCoreCount()

        val html = """
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
                <h1>Manta Multi-Core Rust Engine ($cores Cores)</h1>
                <div class="box">
                    <p id="bench">Booting Multi-Core Worker Pool...</p>
                    <p>Full Chrome-Grade Rendering • Auto Cookie-Banner Zapper • Cloudflare Turnstile Ready</p>
                </div>
                <h2>Tap any link below to launch:</h2>
                <a href="manta://cors-lab">Open Interactive Developer CORS & Bot Lab</a>
                <a href="https://duckduckgo.com">Launch DuckDuckGo Search</a>
                <a href="https://www.google.com">Launch Google Search</a>
                <a href="https://github.com">Launch GitHub</a>
                <script>
                    document.title = "Manta Ready ($cores Cores)";
                    document.getElementById("bench").textContent = "$cores Rust Worker Cores Active • Multi-Tab RAM Compressor Ready";
                </script>
            </body>
            </html>
        """.trimIndent()

        val wire = MantaNative.streamHtmlChunk(getActiveTabId(), schemeUrl, html, true)
        val parsed = DisplayListParser.parse(wire)
        rustRenderView.setFrame(parsed)
        onStateUpdated(parsed.title, schemeUrl)
        renderTabStrip()
    }

    private fun streamRemoteUrlViaRust(cleanUrl: String) {
        val tabId = getActiveTabId()
        onStateUpdated("Rust Reader Streaming...", cleanUrl)

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
                        EphemeralWebSandbox.buildCleanChromeUserAgent(context)
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
                    rustRenderView.setFrame(parsed)
                    onStateUpdated(parsed.title, cleanUrl)
                }
            } catch (_: Throwable) {
                post {
                    setReaderMode(false)
                    getOrCreateSandboxForTab(tabId)?.loadUrl(cleanUrl)
                }
            }
        }
    }
}
