package app.manta.browser.ui.screens

import android.content.Context
import android.graphics.Typeface
import android.text.TextUtils
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.widget.FrameLayout
import android.widget.GridLayout
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import app.manta.browser.core.MantaNative
import app.manta.browser.ui.theme.MantaTheme
import kotlin.math.abs

data class EphemeralTabInfo(
    val id: Int,
    var title: String,
    var url: String
)

/**
 * M3 Expressive Tab Chooser Grid Screen (`LibraryTabsScreen.kt`):
 * - Borderless 2-column squircle tab cards with tonal elevation.
 * - Supports horizontal swipe-to-dismiss gestures on any tab card!
 * - Floating bottom pill action bar with `+ New Tab`, `Zeroize All`, and `Done`.
 */
class LibraryTabsScreen(
    context: Context,
    private val tabs: MutableList<EphemeralTabInfo>,
    private val getActiveTabId: () -> Int,
    private val onSelectTab: (Int) -> Unit,
    private val onCreateNewTab: () -> Unit,
    private val onCloseTab: (Int) -> Unit,
    private val onZeroizeAll: () -> Unit,
    private val onDismissSheet: () -> Unit
) : FrameLayout(context) {

    private val tabsGrid: GridLayout
    private val ramSubheader: TextView

    init {
        setBackgroundColor(MantaTheme.BG_OBSIDIAN)
        visibility = View.GONE

        val scrollView = ScrollView(context).apply {
            isVerticalScrollBarEnabled = false
            layoutParams = LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT)
        }

        val contentCol = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            val padH = MantaTheme.dp(context, 16f)
            setPadding(padH, MantaTheme.dp(context, 28f), padH, MantaTheme.dp(context, 120f))
        }

        // 1. Minimalist Header
        val headerRow = LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            val pH = MantaTheme.dp(context, 6f)
            setPadding(pH, 0, pH, MantaTheme.dp(context, 18f))
        }

        val titleCol = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
        }

        titleCol.addView(TextView(context).apply {
            text = "Open Tabs"
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 24f
            typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
        })

        ramSubheader = TextView(context).apply {
            text = "0.0 KB Volatile RAM • Swipe card to close"
            setTextColor(MantaTheme.TEXT_SECONDARY)
            textSize = 12.5f
        }
        titleCol.addView(ramSubheader)
        headerRow.addView(titleCol)

        val zeroizePill = TextView(context).apply {
            text = "Zeroize All"
            setTextColor(MantaTheme.ON_CONTAINER_TERTIARY)
            textSize = 12.5f
            typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
            val pH = MantaTheme.dp(context, 16f)
            val pV = MantaTheme.dp(context, 10f)
            setPadding(pH, pV, pH, pV)
            background = MantaTheme.roundedBg(MantaTheme.CONTAINER_TERTIARY, 20f, context)
            setOnClickListener { onZeroizeAll() }
        }
        headerRow.addView(zeroizePill)
        contentCol.addView(headerRow)

        // 2. 2-Column M3 Expressive Tab Cards Grid
        tabsGrid = GridLayout(context).apply {
            columnCount = 2
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            )
        }
        contentCol.addView(tabsGrid)
        scrollView.addView(contentCol)
        addView(scrollView)

        // 3. Floating Bottom Expressive Pill Bar (`+ New Tab` | `Done`)
        val bottomBar = LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER
            val margin = MantaTheme.dp(context, 18f)
            layoutParams = LayoutParams(
                LayoutParams.MATCH_PARENT,
                LayoutParams.WRAP_CONTENT,
                Gravity.BOTTOM
            ).apply {
                setMargins(margin, 0, margin, margin)
            }
        }

        val newTabFab = TextView(context).apply {
            text = "+  New Tab"
            setTextColor(MantaTheme.BG_OBSIDIAN)
            textSize = 14.5f
            typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            gravity = Gravity.CENTER
            val pH = MantaTheme.dp(context, 28f)
            val pV = MantaTheme.dp(context, 15f)
            setPadding(pH, pV, pH, pV)
            background = MantaTheme.roundedBg(MantaTheme.ACCENT_PRIMARY, 28f, context)
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f).apply {
                marginEnd = MantaTheme.dp(context, 10f)
            }
            setOnClickListener { onCreateNewTab() }
        }
        bottomBar.addView(newTabFab)

        val donePill = TextView(context).apply {
            text = "Done"
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 14.5f
            typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
            gravity = Gravity.CENTER
            val pH = MantaTheme.dp(context, 24f)
            val pV = MantaTheme.dp(context, 15f)
            setPadding(pH, pV, pH, pV)
            background = MantaTheme.roundedBg(MantaTheme.BG_CONTAINER_HIGH, 28f, context)
            setOnClickListener { onDismissSheet() }
        }
        bottomBar.addView(donePill)
        addView(bottomBar)
    }

    fun refreshLibraryState(logs: List<String>) {
        val ramBytes = MantaNative.getEngineRamBytes()
        ramSubheader.text = String.format(
            "%.1f KB Volatile RAM • %d Shield Events • Swipe card to close",
            ramBytes / 1024f,
            logs.size
        )

        tabsGrid.removeAllViews()
        val activeId = getActiveTabId()

        for (tab in tabs) {
            val isActive = tab.id == activeId
            val card = makeExpressiveTabCard(tab, isActive)
            tabsGrid.addView(card)
        }
    }

    private fun makeExpressiveTabCard(tab: EphemeralTabInfo, isActive: Boolean): View {
        val cardColor = if (isActive) MantaTheme.CONTAINER_PRIMARY else MantaTheme.BG_CARD
        val card = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            val p = MantaTheme.dp(context, 16f)
            setPadding(p, p, p, p)
            background = MantaTheme.roundedBg(cardColor, 26f, context)
            val m = MantaTheme.dp(context, 6f)
            layoutParams = GridLayout.LayoutParams().apply {
                width = 0
                height = MantaTheme.dp(context, 164f)
                columnSpec = GridLayout.spec(GridLayout.UNDEFINED, 1f)
                setMargins(m, m, m, m)
            }
        }

        // Top Row inside Card: Domain Monogram Pill + Close `✕`
        val topRow = LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
        }

        val cleanHost = MantaTheme.extractCleanHost(tab.url)
        val monogram = cleanHost.firstOrNull()?.uppercaseChar()?.toString() ?: "M"

        val badge = TextView(context).apply {
            text = monogram
            setTextColor(if (isActive) MantaTheme.ON_CONTAINER_PRIMARY else MantaTheme.ACCENT_PRIMARY)
            textSize = 12f
            typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            gravity = Gravity.CENTER
            background = MantaTheme.roundedBg(
                if (isActive) MantaTheme.BG_OBSIDIAN else MantaTheme.BG_CONTAINER_HIGH,
                12f,
                context
            )
            val s = MantaTheme.dp(context, 28f)
            layoutParams = LinearLayout.LayoutParams(s, s)
        }
        topRow.addView(badge)

        val hostLabel = TextView(context).apply {
            text = cleanHost
            setTextColor(if (isActive) MantaTheme.ON_CONTAINER_PRIMARY else MantaTheme.TEXT_SECONDARY)
            textSize = 11.5f
            maxLines = 1
            ellipsize = TextUtils.TruncateAt.END
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f).apply {
                marginStart = MantaTheme.dp(context, 8f)
            }
        }
        topRow.addView(hostLabel)

        val closeBtn = TextView(context).apply {
            text = "✕"
            setTextColor(if (isActive) MantaTheme.ON_CONTAINER_PRIMARY else MantaTheme.TEXT_SECONDARY)
            textSize = 13f
            gravity = Gravity.CENTER
            val s = MantaTheme.dp(context, 26f)
            layoutParams = LinearLayout.LayoutParams(s, s)
            setOnClickListener { onCloseTab(tab.id) }
        }
        topRow.addView(closeBtn)
        card.addView(topRow)

        // Middle: Page Title
        val titleTv = TextView(context).apply {
            text = tab.title.ifBlank { "New Tab" }
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 15f
            typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
            maxLines = 3
            ellipsize = TextUtils.TruncateAt.END
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                0,
                1f
            ).apply {
                topMargin = MantaTheme.dp(context, 14f)
            }
        }
        card.addView(titleTv)

        // Bottom: Status Pill (`Active` or `Hibernated in RAM`)
        val statusPill = TextView(context).apply {
            text = if (isActive) "Active Tab" else "Saved in Rust RAM"
            setTextColor(if (isActive) MantaTheme.ACCENT_EMERALD else MantaTheme.TEXT_MUTED)
            textSize = 11f
        }
        card.addView(statusPill)

        // Attach Swipe-to-Dismiss & Tap Gesture Listener to Card
        attachSwipeToDismissGesture(card, tab.id)
        return card
    }

    private fun attachSwipeToDismissGesture(card: View, tabId: Int) {
        var downX = 0f
        var downY = 0f
        var swiping = false

        card.setOnTouchListener { v, ev ->
            when (ev.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    downX = ev.rawX
                    downY = ev.rawY
                    swiping = false
                    true
                }
                MotionEvent.ACTION_MOVE -> {
                    val dx = ev.rawX - downX
                    val dy = ev.rawY - downY
                    if (abs(dx) > MantaTheme.dpF(context, 14f) && abs(dx) > abs(dy)) {
                        swiping = true
                        v.parent?.requestDisallowInterceptTouchEvent(true)
                    }
                    if (swiping) {
                        v.translationX = dx
                        v.alpha = (1f - abs(dx) / (v.width.coerceAtLeast(200) * 0.9f)).coerceIn(0.2f, 1f)
                    }
                    true
                }
                MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                    val dx = ev.rawX - downX
                    val dy = ev.rawY - downY
                    if (swiping && abs(dx) > v.width * 0.38f) {
                        onCloseTab(tabId)
                    } else {
                        if (!swiping && abs(dx) < MantaTheme.dpF(context, 10f) && abs(dy) < MantaTheme.dpF(context, 10f)) {
                            onSelectTab(tabId)
                        }
                        v.animate().translationX(0f).alpha(1f).setDuration(180L).start()
                    }
                    true
                }
                else -> false
            }
        }
    }
}
