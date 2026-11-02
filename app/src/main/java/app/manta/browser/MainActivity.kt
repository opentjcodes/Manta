package app.manta.browser

import android.app.Activity
import android.os.Bundle
import android.view.Gravity
import android.view.View
import android.widget.FrameLayout
import android.widget.Toast
import app.manta.browser.core.MantaNative
import app.manta.browser.core.ZeroDiskJanitor
import app.manta.browser.ui.components.MiniStatusPill
import app.manta.browser.ui.components.SlidingBottomDock
import app.manta.browser.ui.screens.EphemeralTabInfo
import app.manta.browser.ui.screens.LibraryTabsScreen
import app.manta.browser.ui.screens.SearchBrowserScreen
import app.manta.browser.ui.theme.MantaTheme
import java.util.concurrent.CopyOnWriteArrayList

/**
 * Manta Main Activity:
 * - Full-screen edge-to-edge browser viewport (`SearchBrowserScreen`).
 * - Single auto-hiding M3 Expressive Gesture Dock (`SlidingBottomDock`) at the bottom.
 * - Floating Google Omnibox & Chrome Controls Sheet (`MiniStatusPill`).
 * - Full-screen M3 Expressive 2-Column Tab Chooser Grid (`LibraryTabsScreen`).
 * - Zero disk cache bloat + Clearable Custom Rust Storage (`manta_custom_storage.bin`).
 */
class MainActivity : Activity() {

    private val openTabs = CopyOnWriteArrayList<EphemeralTabInfo>()
    private val securityLogs = CopyOnWriteArrayList<String>()
    private var activeTabId: Int = 1
    private var desktopModeEnabled: Boolean = false
    private var activeLoadProgress: Int = 100

    private var searchScreen: SearchBrowserScreen? = null
    private var tabChooserScreen: LibraryTabsScreen? = null
    private var omniboxSheet: MiniStatusPill? = null
    private var bottomDock: SlidingBottomDock? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val customStoragePath = ZeroDiskJanitor.getCustomStorageFilePath(this)
        activeTabId = MantaNative.initEngine(customStoragePath)
        openTabs.add(
            EphemeralTabInfo(
                id = activeTabId,
                title = "New Tab",
                url = "manta://start"
            )
        )

        val rootContainer = FrameLayout(this).apply {
            setBackgroundColor(MantaTheme.BG_OBSIDIAN)
        }

        // 1. Construct Floating Gesture Bottom Dock
        val dock = SlidingBottomDock(
            context = this,
            onAddressBarTap = {
                val curUrl = searchScreen?.currentUrl ?: ""
                omniboxSheet?.showOmnibox(curUrl, focusKeyboard = true)
            },
            onTabSwitcherTap = {
                openTabChooser()
            },
            onMenuTap = {
                val curUrl = searchScreen?.currentUrl ?: ""
                omniboxSheet?.showOmnibox(curUrl, focusKeyboard = false)
            },
            onHorizontalTabSwipe = { direction ->
                searchScreen?.cycleTabByOffset(direction)
                syncDockAndTelemetry()
            },
            onBackTap = {
                if (searchScreen?.handleBackNavigation() != true) {
                    searchScreen?.loadInputUrlOrQuery("manta://start", false)
                }
                syncDockAndTelemetry()
            }
        )
        bottomDock = dock

        // 2. Construct Floating Omnibox & Chrome Controls Sheet
        val omni = MiniStatusPill(
            context = this,
            onNavigateToUrlOrSearch = { queryOrUrl, forceReader ->
                searchScreen?.loadInputUrlOrQuery(queryOrUrl, forceReader)
                syncDockAndTelemetry()
            },
            onReloadPage = {
                searchScreen?.reloadCurrentPage()
            },
            onToggleReaderMode = {
                searchScreen?.toggleReaderMode()
            },
            onToggleDesktopMode = { enabled ->
                desktopModeEnabled = enabled
                searchScreen?.setDesktopSiteMode(enabled)
                securityLogs.add("Desktop Site Mode: ${if (enabled) "ON" else "OFF"}")
                syncDockAndTelemetry()
            },
            onZeroizeRam = {
                performClearCustomStorageAndRam()
            }
        )
        omniboxSheet = omni

        // 3. Construct Full-Screen Browser Viewport
        val browser = SearchBrowserScreen(
            context = this,
            openTabs = openTabs,
            getActiveTabId = { activeTabId },
            setActiveTabId = { newId -> activeTabId = newId },
            isDesktopModeEnabled = { desktopModeEnabled },
            onStateUpdated = { title, url, progress ->
                activeLoadProgress = progress
                openTabs.find { it.id == activeTabId }?.let {
                    it.title = title
                    it.url = url
                }
                syncDockAndTelemetry()
            },
            onScrollDirectionChanged = { scrollingDown ->
                dock.setDockVisible(!scrollingDown)
            },
            onOpenOmnibox = {
                val curUrl = searchScreen?.currentUrl ?: ""
                omni.showOmnibox(curUrl, focusKeyboard = true)
            },
            onSecurityLog = { event ->
                securityLogs.add(event)
            }
        )
        searchScreen = browser

        // 4. Construct M3 Expressive 2-Column Tab Chooser Screen
        val tabChooser = LibraryTabsScreen(
            context = this,
            tabs = openTabs,
            getActiveTabId = { activeTabId },
            onSelectTab = { tabId ->
                closeTabChooser()
                browser.switchToExistingTab(tabId)
                syncDockAndTelemetry()
            },
            onCreateNewTab = {
                closeTabChooser()
                browser.createNewTabAndSwitch("manta://start")
                syncDockAndTelemetry()
            },
            onCloseTab = { tabId ->
                browser.closeTabById(tabId)
                tabChooserScreen?.refreshLibraryState(securityLogs)
                syncDockAndTelemetry()
            },
            onZeroizeAll = {
                closeTabChooser()
                performClearCustomStorageAndRam()
            },
            onDismissSheet = {
                closeTabChooser()
            }
        )
        tabChooserScreen = tabChooser

        // Assemble layers: Fullscreen Browser -> Floating Dock -> Omnibox Overlay -> Tab Chooser Overlay
        rootContainer.addView(
            browser,
            FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT
            )
        )

        val dockMarginH = MantaTheme.dp(this, 14f)
        val dockMarginB = MantaTheme.dp(this, 12f)
        rootContainer.addView(
            dock,
            FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                MantaTheme.dp(this, 56f),
                Gravity.BOTTOM
            ).apply {
                setMargins(dockMarginH, 0, dockMarginH, dockMarginB)
            }
        )

        rootContainer.addView(
            omni,
            FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT
            )
        )

        rootContainer.addView(
            tabChooser,
            FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT
            )
        )

        setContentView(rootContainer)

        browser.bootInitialPage()
        syncDockAndTelemetry()
    }

    private fun openTabChooser() {
        omniboxSheet?.dismissOverlay()
        tabChooserScreen?.refreshLibraryState(securityLogs)
        tabChooserScreen?.apply {
            alpha = 0f
            translationY = MantaTheme.dpF(context, 40f)
            visibility = View.VISIBLE
            animate().alpha(1f).translationY(0f).setDuration(200L).start()
        }
    }

    private fun closeTabChooser() {
        tabChooserScreen?.visibility = View.GONE
    }

    private fun syncDockAndTelemetry() {
        val activeTab = openTabs.find { it.id == activeTabId }
        val urlOrTitle = activeTab?.url ?: "manta://start"
        val ramBytes = MantaNative.getEngineRamBytes()

        bottomDock?.updateDockState(
            urlOrTitle = urlOrTitle,
            tabsCount = openTabs.size,
            progress = activeLoadProgress,
            isCorsEnabled = desktopModeEnabled
        )

        omniboxSheet?.updateStatus(
            title = activeTab?.title ?: "Manta",
            ramBytes = ramBytes,
            isDesktopOn = desktopModeEnabled,
            tabCount = openTabs.size
        )
    }

    private fun performClearCustomStorageAndRam() {
        searchScreen?.wipeAllTabsAndSandboxes()
        ZeroDiskJanitor.purgeAllDiskAndRamTraces(this)
        openTabs.clear()
        securityLogs.clear()

        val customStoragePath = ZeroDiskJanitor.getCustomStorageFilePath(this)
        activeTabId = MantaNative.initEngine(customStoragePath)
        openTabs.add(EphemeralTabInfo(activeTabId, "New Tab", "manta://start"))
        searchScreen?.bootInitialPage()
        tabChooserScreen?.refreshLibraryState(securityLogs)
        syncDockAndTelemetry()
        Toast.makeText(
            this,
            "Cleared Custom Storage Vault, Cookies & Tabs (0 B)",
            Toast.LENGTH_SHORT
        ).show()
    }

    @Deprecated("Deprecated in Java")
    override fun onBackPressed() {
        if (omniboxSheet?.visibility == View.VISIBLE) {
            omniboxSheet?.dismissOverlay()
            return
        }
        if (tabChooserScreen?.visibility == View.VISIBLE) {
            closeTabChooser()
            return
        }
        if (searchScreen?.handleBackNavigation() == true) {
            syncDockAndTelemetry()
            return
        }
        ZeroDiskJanitor.enforceZeroCacheAndFlushCustomStorage(this)
        super.onBackPressed()
    }

    override fun onStop() {
        super.onStop()
        ZeroDiskJanitor.enforceZeroCacheAndFlushCustomStorage(this)
    }

    override fun onDestroy() {
        ZeroDiskJanitor.enforceZeroCacheAndFlushCustomStorage(this)
        super.onDestroy()
    }
}
