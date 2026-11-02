package app.manta.browser.ui.screens

import android.content.Context
import android.view.View
import android.widget.FrameLayout
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
 * Full-Screen Edge-to-Edge Browser Viewport (`SearchBrowserScreen.kt`):
 * - Zero top bars, zero borders, 100% screen space dedicated to the website!
 * - Uses Google (`https://www.google.com/search?q=...`) as the default search engine.
 * - Detects vertical scroll direction to automatically hide/reveal the floating bottom dock.
 */
class SearchBrowserScreen(
    context: Context,
    private val openTabs: MutableList<EphemeralTabInfo>,
    private val getActiveTabId: () -> Int,
    private val setActiveTabId: (Int) -> Unit,
    private val isCorsDevEnabled: () -> Boolean,
    private val onStateUpdated: (title: String, url: String, progress: Int) -> Unit,
    private val onScrollDirectionChanged: (scrollingDown: Boolean) -> Unit,
    private val onOpenOmnibox: () -> Unit,
    private val onShieldLog: (String) -> Unit
) : FrameLayout(context) {

    private val startOverlay: HomeScreen
    private val rustRenderView: MantaRenderView
    private val tabSandboxes = HashMap<Int, EphemeralWebSandbox>()
    private val maxLiveSandboxesInRam = 4

    private var useRustReaderMode: Boolean = false
    var currentUrl: String = "manta://start"
        private set
    private var currentProgress: Int = 100

    init {
        setBackgroundColor(MantaTheme.BG_OBSIDIAN)

        // 1. Direct Rust Reader Canvas (Hidden unless Reader Mode is toggled)
        rustRenderView = MantaRenderView(
            context = context,
            onLinkClicked = { href -> loadInputUrlOrQuery(href, false) },
            onTextCopied = { copied ->
                MantaNative.copyToClipboard(copied)
                Toast.makeText(context, "Copied paragraph to Rust Vault", Toast.LENGTH_SHORT).show()
            }
        ).apply {
            visibility = View.GONE
        }
        addView(
            rustRenderView,
            LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT)
        )

        // 2. Minimalist New-Tab Start Overlay (`HomeScreen`)
        startOverlay = HomeScreen(
            context = context,
            onOpenOmniboxSearch = onOpenOmnibox,
            onLaunchUrl = { targetUrl, preferReader ->
                loadInputUrlOrQuery(targetUrl, preferReader)
            }
        )
        addView(
            startOverlay,
            LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT)
        )
    }

    fun bootInitialPage() {
        showStartOverlayForTab(getActiveTabId())
    }

    private fun showStartOverlayForTab(tabId: Int) {
        currentUrl = "manta://start"
        openTabs.find { it.id == tabId }?.let {
            it.title = "New Tab"
            it.url = "manta://start"
        }
        tabSandboxes.values.forEach { it.visibility = View.GONE }
        rustRenderView.visibility = View.GONE
        startOverlay.visibility = View.VISIBLE
        onScrollDirectionChanged(false)
        onStateUpdated("New Tab", "manta://start", 100)
    }

    /**
     * Gesture Tab Switching: Swipe Left/Right on the Bottom Dock to cycle through open tabs.
     */
    fun cycleTabByOffset(offset: Int) {
        if (openTabs.size <= 1) {
            Toast.makeText(context, "1 Tab Open • Swipe up for Tab Switcher", Toast.LENGTH_SHORT).show()
            return
        }
        val curIdx = openTabs.indexOfFirst { it.id == getActiveTabId() }.coerceAtLeast(0)
        val nextIdx = (curIdx + offset + openTabs.size) % openTabs.size
        val targetTab = openTabs[nextIdx]
        switchToExistingTab(targetTab.id)
    }

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
        onScrollDirectionChanged(false)

        val tabInfo = openTabs.find { it.id == targetTabId }
        val targetUrl = tabInfo?.url ?: "manta://start"
        currentUrl = targetUrl

        if (targetUrl == "manta://start") {
            showStartOverlayForTab(targetTabId)
            return
        }

        startOverlay.visibility = View.GONE
        val existingSandbox = tabSandboxes[targetTabId]
        if (existingSandbox != null && !useRustReaderMode) {
            rustRenderView.visibility = View.GONE
            existingSandbox.visibility = View.VISIBLE
            existingSandbox.wakeFromHibernation()
            onStateUpdated(tabInfo?.title ?: "Manta", targetUrl, 100)
        } else {
            loadInputUrlOrQuery(targetUrl, useRustReaderMode)
        }
    }

    fun createNewTabAndSwitch(initialUrl: String = "manta://start") {
        val prevId = getActiveTabId()
        tabSandboxes[prevId]?.let {
            it.hibernateInBackground()
            it.visibility = View.GONE
        }

        val newId = MantaNative.createNewTab()
        openTabs.add(EphemeralTabInfo(newId, "New Tab", initialUrl))
        setActiveTabId(newId)
        MantaNative.switchTab(newId)

        if (initialUrl == "manta://start") {
            showStartOverlayForTab(newId)
        } else {
            loadInputUrlOrQuery(initialUrl, false)
        }
    }

    fun closeTabById(tabId: Int) {
        tabSandboxes.remove(tabId)?.let { sandbox ->
            sandbox.wipeAllEphemeralData()
            removeView(sandbox)
            sandbox.destroy()
        }
        MantaNative.closeTabAndZeroize(tabId)
        openTabs.removeAll { it.id == tabId }

        if (openTabs.isEmpty()) {
            val freshId = MantaNative.createNewTab()
            openTabs.add(EphemeralTabInfo(freshId, "New Tab", "manta://start"))
            setActiveTabId(freshId)
            showStartOverlayForTab(freshId)
        } else if (getActiveTabId() == tabId) {
            val nextTab = openTabs.last()
            switchToExistingTab(nextTab.id)
        }
    }

    fun toggleReaderMode() {
        useRustReaderMode = !useRustReaderMode
        if (currentUrl != "manta://start") {
            loadUrlInternal(currentUrl, forceReload = true)
        }
    }

    /**
     * Resolves any user input to a URL or a Google Search (`https://www.google.com/search?q=...`).
     */
    fun loadInputUrlOrQuery(rawInput: String, preferRustReader: Boolean) {
        val trimmed = rawInput.trim()
        if (trimmed.isEmpty()) return

        if (trimmed == "manta://start") {
            showStartOverlayForTab(getActiveTabId())
            return
        }

        val resolvedUrl = when {
            trimmed.startsWith("manta://") -> trimmed
            trimmed.startsWith("http://") || trimmed.startsWith("https://") -> trimmed
            trimmed.startsWith("localhost") -> "http://$trimmed"
            trimmed.contains(".") && !trimmed.contains(" ") -> "https://$trimmed"
            else -> "https://www.google.com/search?q=${URLEncoder.encode(trimmed, "UTF-8")}"
        }

        useRustReaderMode = preferRustReader
        MantaNative.prewarmDns(resolvedUrl)
        loadUrlInternal(resolvedUrl, forceReload = false)
    }

    fun reloadCurrentPage() {
        if (currentUrl != "manta://start") {
            loadUrlInternal(currentUrl, forceReload = true)
        }
    }

    fun wipeAllTabsAndSandboxes() {
        for ((_, sandbox) in tabSandboxes) {
            sandbox.wipeAllEphemeralData()
            removeView(sandbox)
            sandbox.destroy()
        }
        tabSandboxes.clear()
        showStartOverlayForTab(getActiveTabId())
    }

    fun handleBackNavigation(): Boolean {
        val activeSandbox = tabSandboxes[getActiveTabId()]
        if (startOverlay.visibility != View.VISIBLE && !useRustReaderMode && activeSandbox != null) {
            if (activeSandbox.canGoBack()) {
                activeSandbox.goBack()
                return true
            } else {
                showStartOverlayForTab(getActiveTabId())
                return true
            }
        }
        if (useRustReaderMode && startOverlay.visibility != View.VISIBLE) {
            useRustReaderMode = false
            showStartOverlayForTab(getActiveTabId())
            return true
        }
        return false
    }

    private fun getOrCreateSandboxForTab(tabId: Int): EphemeralWebSandbox? {
        tabSandboxes[tabId]?.let { return it }

        if (tabSandboxes.size >= maxLiveSandboxesInRam) {
            val oldestId = tabSandboxes.keys.firstOrNull { it != tabId }
            if (oldestId != null) {
                tabSandboxes.remove(oldestId)?.let { oldView ->
                    oldView.hibernateInBackground()
                    removeView(oldView)
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
                        if (url.isNotBlank()) currentUrl = url
                        onStateUpdated(title, currentUrl, currentProgress)
                    }
                },
                onProgressUpdate = { updatedTabId, pct ->
                    if (updatedTabId == getActiveTabId()) {
                        currentProgress = pct
                        val activeTab = openTabs.find { it.id == updatedTabId }
                        onStateUpdated(activeTab?.title ?: "Loading...", currentUrl, pct)
                    }
                },
                onShieldEvent = onShieldLog
            )

            // Attach scroll-direction listener so the bottom dock auto-hides for 100% page space
            created.setOnScrollChangeListener { _, _, scrollY, _, oldScrollY ->
                val dy = scrollY - oldScrollY
                if (dy > 18 && scrollY > 120) {
                    onScrollDirectionChanged(true) // Hide dock
                } else if (dy < -18 || scrollY < 60) {
                    onScrollDirectionChanged(false) // Show dock
                }
            }

            addView(
                created,
                0, // Add below overlays so Omnibox/TabChooser always sit above
                LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT)
            )
            tabSandboxes[tabId] = created
            created
        } catch (_: Throwable) {
            null
        }
    }

    private fun loadUrlInternal(targetUrl: String, forceReload: Boolean) {
        val activeId = getActiveTabId()
        startOverlay.visibility = View.GONE
        onScrollDirectionChanged(false)
        openTabs.find { it.id == activeId }?.url = targetUrl

        if (targetUrl == "manta://cors-lab") {
            loadInteractiveCorsLabInFullEngine(activeId)
            return
        }

        val verdict = MantaNative.sanitizeAndCheckUrl(targetUrl)
        if (verdict == "BLOCKED") {
            onShieldLog("Blocked navigation to tracker host: $targetUrl")
            Toast.makeText(context, "Rust Shield blocked tracker domain", Toast.LENGTH_SHORT).show()
            return
        }

        currentUrl = verdict

        if (useRustReaderMode) {
            tabSandboxes.values.forEach { it.visibility = View.GONE }
            rustRenderView.visibility = View.VISIBLE
            streamRemoteUrlViaRust(verdict)
        } else {
            rustRenderView.visibility = View.GONE
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
                if (forceReload || sandbox.url != verdict) {
                    sandbox.loadUrl(verdict)
                }
            }
        }
    }

    private fun loadInteractiveCorsLabInFullEngine(tabId: Int) {
        currentUrl = "manta://cors-lab"
        val sandbox = getOrCreateSandboxForTab(tabId) ?: return
        sandbox.visibility = View.VISIBLE
        rustRenderView.visibility = View.GONE

        val labHtml = """
            <!DOCTYPE html>
            <html>
            <head>
                <meta name="viewport" content="width=device-width, initial-scale=1.0">
                <title>Manta CORS & Turnstile Lab</title>
                <style>
                    body { background:#090a0f; color:#f1f5f9; font-family:sans-serif; padding:20px; margin:0; }
                    .card { background:#191c26; border-radius:24px; padding:18px; margin-bottom:16px; }
                    h2 { margin:0 0 8px 0; font-size:17px; color:#a8c7fa; }
                    p { margin:4px 0; font-size:13.5px; color:#94a3b8; }
                    button { background:#a8c7fa; color:#090a0f; border:none; border-radius:18px; padding:11px 16px; font-weight:bold; font-size:13px; margin-top:10px; margin-right:8px; }
                    pre { background:#12141c; color:#6dd58c; padding:12px; border-radius:16px; font-size:12px; overflow-x:auto; white-space:pre-wrap; }
                </style>
            </head>
            <body>
                <div class="card">
                    <h2>Live Developer CORS Tester</h2>
                    <p>Verify cross-origin GET & POST requests through Manta's Rust CORS Engine:</p>
                    <button onclick="runCorsGet()">Test Cross-Origin GET</button>
                    <button onclick="runCorsPost()">Test Cross-Origin POST</button>
                    <pre id="cors-out">Ready to test cross-origin requests...</pre>
                </div>
                <div class="card">
                    <h2>Cloudflare Turnstile & Stealth Status</h2>
                    <pre id="bot-out"></pre>
                </div>
                <script>
                    document.getElementById('bot-out').textContent =
                        'navigator.webdriver: ' + navigator.webdriver + '\n' +
                        'User-Agent: ' + navigator.userAgent + '\n' +
                        'Session Cookies Ready: ' + navigator.cookieEnabled;

                    async function runCorsGet() {
                        const out = document.getElementById('cors-out');
                        out.textContent = 'Fetching https://httpbin.org/get ...';
                        try {
                            const res = await fetch('https://httpbin.org/get?manta=1');
                            const txt = await res.text();
                            out.textContent = 'HTTP ' + res.status + ':\n' + txt.slice(0, 400);
                        } catch (e) { out.textContent = 'Error: ' + e; }
                    }

                    async function runCorsPost() {
                        const out = document.getElementById('cors-out');
                        out.textContent = 'POSTing to https://httpbin.org/post ...';
                        try {
                            const res = await fetch('https://httpbin.org/post', {
                                method: 'POST',
                                headers: { 'Content-Type': 'application/json' },
                                body: JSON.stringify({ browser: 'Manta', corsUnlocked: true })
                            });
                            const txt = await res.text();
                            out.textContent = 'HTTP ' + res.status + ':\n' + txt.slice(0, 420);
                        } catch (e) { out.textContent = 'Error: ' + e; }
                    }
                </script>
            </body>
            </html>
        """.trimIndent()

        sandbox.loadDataWithBaseURL("https://manta.local/", labHtml, "text/html", "UTF-8", null)
        onStateUpdated("CORS & Turnstile Lab", "manta://cors-lab", 100)
    }

    private fun streamRemoteUrlViaRust(cleanUrl: String) {
        val tabId = getActiveTabId()
        onStateUpdated("Reader Mode...", cleanUrl, 40)

        thread(start = true) {
            try {
                val conn = (URL(cleanUrl).openConnection() as HttpURLConnection).apply {
                    requestMethod = "GET"
                    connectTimeout = 6000
                    readTimeout = 8000
                    useCaches = false
                    setRequestProperty("User-Agent", EphemeralWebSandbox.buildCleanChromeUserAgent(context))
                }
                val stream = conn.inputStream
                val buf = ByteArray(4096)
                var totalRead = 0
                while (true) {
                    val n = stream.read(buf)
                    if (n <= 0) break
                    totalRead += n
                    MantaNative.streamHtmlChunk(tabId, cleanUrl, String(buf, 0, n, Charsets.UTF_8), false)
                    if (totalRead > 256 * 1024) break
                }
                stream.close()

                val finalWire = MantaNative.streamHtmlChunk(tabId, cleanUrl, "", true)
                val parsed = DisplayListParser.parse(finalWire)
                post {
                    rustRenderView.setFrame(parsed)
                    onStateUpdated(parsed.title, cleanUrl, 100)
                }
            } catch (_: Throwable) {
                post {
                    useRustReaderMode = false
                    loadUrlInternal(cleanUrl, forceReload = true)
                }
            }
        }
    }
}
