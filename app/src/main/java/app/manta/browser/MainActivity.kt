package app.manta.browser

import android.app.Activity
import android.os.Bundle
import android.view.Gravity
import android.view.View
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.Toast
import app.manta.browser.core.MantaNative
import app.manta.browser.ui.components.MiniStatusPill
import app.manta.browser.ui.components.SlidingBottomDock
import app.manta.browser.ui.screens.EphemeralTabInfo
import app.manta.browser.ui.screens.HomeScreen
import app.manta.browser.ui.screens.LibraryTabsScreen
import app.manta.browser.ui.screens.SearchBrowserScreen
import app.manta.browser.ui.theme.MantaTheme

/**
 * Manta Main Activity:
 * Hosts the 3 workspace screens (Home | Search | Library), the floating Muse-style
 * `MiniStatusPill`, and the draggable/slidable `SlidingBottomDock`.
 * Automatically zeroizes all volatile RAM in Rust when the user closes the app.
 */
class MainActivity : Activity() {

    private val openTabs = mutableListOf<EphemeralTabInfo>()
    private val shieldLogs = mutableListOf<String>()
    private var activeTabId: Int = 1
    private var corsDevPermissive: Boolean = true
    private var currentDockSection: Int = 0

    private lateinit var homeScreen: HomeScreen
    private lateinit var searchScreen: SearchBrowserScreen
    private lateinit var libraryScreen: LibraryTabsScreen
    private lateinit var miniStatusPill: MiniStatusPill
    private lateinit var bottomDock: SlidingBottomDock

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Initialize Rust Engine (`libmanta_core.so`) with Developer CORS enabled
        activeTabId = MantaNative.initEngine(corsDevPermissive)
        openTabs.add(
            EphemeralTabInfo(
                id = activeTabId,
                title = "Manta Rust Core",
                url = "manta://benchmark"
            )
        )

        val rootContainer = FrameLayout(this).apply {
            setBackgroundColor(MantaTheme.BG_OBSIDIAN)
        }

        // 1. Initialize the 3 Dock Screens
        homeScreen = HomeScreen(
            context = this,
            onLaunchUrl = { url, preferRustStream ->
                switchDockSection(1, animateDock = true)
                searchScreen.loadInputUrlOrQuery(url, preferRustStream)
            },
            onToggleCorsDev = { enabled ->
                corsDevPermissive = enabled
                MantaNative.setCorsDevMode(enabled)
                shieldLogs.add("Developer CORS Mode set to: ${if (enabled) "OPEN" else "STRICT"}")
                refreshStatusPill()
            },
            onTriggerZeroize = {
                performInstantZeroizePurge()
            }
        )

        searchScreen = SearchBrowserScreen(
            context = this,
            getActiveTabId = { activeTabId },
            isCorsDevEnabled = { corsDevPermissive },
            onStateUpdated = { title, url ->
                openTabs.find { it.id == activeTabId }?.let {
                    it.title = title
                    it.url = url
                }
                refreshStatusPill()
            },
            onShieldLog = { event ->
                shieldLogs.add(event)
            }
        )

        libraryScreen = LibraryTabsScreen(
            context = this,
            tabs = openTabs,
            getActiveTabId = { activeTabId },
            onSelectTab = { tabId ->
                activeTabId = tabId
                MantaNative.switchTab(tabId)
                val tab = openTabs.find { it.id == tabId }
                if (tab != null) {
                    searchScreen.loadInputUrlOrQuery(tab.url, tab.url.startsWith("manta://"))
                }
                switchDockSection(1, animateDock = true)
            },
            onCreateNewTab = {
                val newId = MantaNative.createNewTab()
                activeTabId = newId
                openTabs.add(EphemeralTabInfo(newId, "New Manta Tab", "manta://benchmark"))
                searchScreen.loadInputUrlOrQuery("manta://benchmark", true)
                switchDockSection(1, animateDock = true)
            },
            onCloseTab = { tabId ->
                MantaNative.closeTabAndZeroize(tabId)
                openTabs.removeAll { it.id == tabId }
                if (openTabs.isEmpty()) {
                    val freshId = MantaNative.createNewTab()
                    openTabs.add(EphemeralTabInfo(freshId, "Manta Rust Core", "manta://benchmark"))
                }
                activeTabId = openTabs.last().id
                MantaNative.switchTab(activeTabId)
                libraryScreen.refreshLibraryState(shieldLogs)
                refreshStatusPill()
            },
            onZeroizeAll = {
                performInstantZeroizePurge()
            }
        )

        rootContainer.addView(homeScreen)
        rootContainer.addView(searchScreen)
        rootContainer.addView(libraryScreen)

        // 2. Bottom Floating Stack (Muse-style MiniStatusPill + SlidingBottomDock)
        val bottomFloatingStack = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            val marginH = MantaTheme.dp(this@MainActivity, 12f)
            val marginB = MantaTheme.dp(this@MainActivity, 10f)
            layoutParams = FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.WRAP_CONTENT,
                Gravity.BOTTOM
            ).apply {
                setMargins(marginH, 0, marginH, marginB)
            }
        }

        miniStatusPill = MiniStatusPill(
            context = this,
            onOpenActivePage = { switchDockSection(1, animateDock = true) },
            onReloadPage = { searchScreen.reloadCurrentPage() },
            onZeroizeRam = { performInstantZeroizePurge() }
        )
        bottomFloatingStack.addView(
            miniStatusPill,
            LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                bottomMargin = MantaTheme.dp(this@MainActivity, 8f)
            }
        )

        bottomDock = SlidingBottomDock(this) { sectionIndex ->
            switchDockSection(sectionIndex, animateDock = false)
        }
        bottomFloatingStack.addView(
            bottomDock,
            LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                MantaTheme.dp(this, 60f)
            )
        )

        rootContainer.addView(bottomFloatingStack)
        setContentView(rootContainer)

        switchDockSection(0, animateDock = false)
        refreshStatusPill()
    }

    private fun switchDockSection(index: Int, animateDock: Boolean) {
        currentDockSection = index
        homeScreen.visibility = if (index == 0) View.VISIBLE else View.GONE
        searchScreen.visibility = if (index == 1) View.VISIBLE else View.GONE
        libraryScreen.visibility = if (index == 2) View.VISIBLE else View.GONE

        if (index == 2) {
            libraryScreen.refreshLibraryState(shieldLogs)
        }
        if (animateDock) {
            bottomDock.setSelectedSection(index, animate = true)
        }
        refreshStatusPill()
    }

    private fun refreshStatusPill() {
        val activeTab = openTabs.find { it.id == activeTabId }
        val ramBytes = MantaNative.getEngineRamBytes()
        miniStatusPill.updateStatus(
            title = activeTab?.title ?: "Manta Rust Core",
            ramBytes = ramBytes,
            corsEnabled = corsDevPermissive,
            tabCount = openTabs.size
        )
    }

    private fun performInstantZeroizePurge() {
        MantaNative.purgeAllVolatileRam()
        searchScreen.wipeSandboxMemory()
        openTabs.clear()
        shieldLogs.clear()

        activeTabId = MantaNative.initEngine(corsDevPermissive)
        openTabs.add(EphemeralTabInfo(activeTabId, "Manta Zeroized", "manta://benchmark"))
        libraryScreen.refreshLibraryState(shieldLogs)
        refreshStatusPill()
        Toast.makeText(
            this,
            "All Volatile RAM & Ephemeral Tabs Zeroized (0 KB)",
            Toast.LENGTH_SHORT
        ).show()
    }

    @Deprecated("Deprecated in Java")
    override fun onBackPressed() {
        if (currentDockSection == 1 && searchScreen.handleBackNavigation()) {
            return
        }
        if (currentDockSection != 0) {
            switchDockSection(0, animateDock = true)
            return
        }
        MantaNative.purgeAllVolatileRam()
        super.onBackPressed()
    }

    override fun onStop() {
        super.onStop()
        if (isFinishing) {
            MantaNative.purgeAllVolatileRam()
        }
    }

    override fun onDestroy() {
        MantaNative.purgeAllVolatileRam()
        super.onDestroy()
    }
}
