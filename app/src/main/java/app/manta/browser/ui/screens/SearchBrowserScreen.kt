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
 * - Zero top bars, zero borders, 100% screen space dedicated to the website.
 * - Uses Google (`https://www.google.com/search?q=...`) as the default search engine.
 * - Enforces per-origin Site Isolation and custom clearable Rust storage.
 */
class SearchBrowserScreen(
    context: Context,
    private val openTabs: MutableList<EphemeralTabInfo>,
    private val getActiveTabId: () -> Int,
    private val setActiveTabId: (Int) -> Unit,
    private val isDesktopModeEnabled: () -> Boolean,
    private val onStateUpdated: (title: String, url: String, progress: Int) -> Unit,
    private val onScrollDirectionChanged: (scrollingDown: Boolean) -> Unit,
    private val onOpenOmnibox: () -> Unit,
    private val onSecurityLog: (String) -> Unit
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
                Toast.makeText(context, "Copied paragraph to clipboard", Toast.LENGTH_SHORT).show()
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
            sandbox.releaseTabResources()
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

    fun setDesktopSiteMode(enabled: Boolean) {
        val activeId = getActiveTabId()
        MantaNative.setDesktopMode(activeId, enabled)
        tabSandboxes[activeId]?.let { sandbox ->
            sandbox.applyDesktopModeSetting(enabled)
            if (currentUrl != "manta://start") {
                sandbox.reload()
            }
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
            trimmed.startsWith("localhost") || trimmed.startsWith("127.0.0.1") -> "http://$trimmed"
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
                isDesktopModeEnabled = isDesktopModeEnabled,
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
                onSecurityEvent = onSecurityLog
            )

            created.setOnScrollChangeListener { _, _, scrollY, _, oldScrollY ->
                val dy = scrollY - oldScrollY
                if (dy > 18 && scrollY > 120) {
                    onScrollDirectionChanged(true)
                } else if (dy < -18 || scrollY < 60) {
                    onScrollDirectionChanged(false)
                }
            }

            addView(
                created,
                0,
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

        val verdict = MantaNative.validateUrl("", targetUrl, true)
        if (verdict == "BLOCKED") {
            onSecurityLog("Blocked unsafe navigation: $targetUrl")
            Toast.makeText(context, "Site Isolation blocked unsafe URL scheme", Toast.LENGTH_SHORT).show()
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
                    setRequestProperty(
                        "User-Agent",
                        EphemeralWebSandbox.buildCleanChromeUserAgent(context, isDesktopModeEnabled())
                    )
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
