package app.manta.browser.ui.screens

import android.content.Context
import android.graphics.Typeface
import android.view.Gravity
import android.widget.HorizontalScrollView
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import app.manta.browser.ui.theme.MantaTheme

data class QuickLaunchSite(
    val title: String,
    val subtitle: String,
    val url: String,
    val accentColor: Int,
    val badge: String
)

/**
 * Muse-Inspired Home Screen:
 * - Bold `MANTA` brand bar + top-right developer controls icon
 * - Horizontal scrollable filter pills (like `All Curated`, `Malayalam`, `Hindi` in Muse)
 * - Featured Collections & Dev Sandboxes (large gradient/accent rounded cards with Play/Launch badge)
 * - Speed Dial & Privacy Verification cards
 */
class HomeScreen(
    context: Context,
    private val onLaunchUrl: (url: String, preferRustStream: Boolean) -> Unit,
    private val onToggleCorsDev: (Boolean) -> Unit,
    private val onTriggerZeroize: () -> Unit
) : ScrollView(context) {

    private var corsEnabled = true
    private var selectedCategoryIndex = 0
    private val categoryChipViews = ArrayList<TextView>()

    init {
        setBackgroundColor(MantaTheme.BG_OBSIDIAN)
        isVerticalScrollBarEnabled = false

        val root = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            val padH = MantaTheme.dp(context, 18f)
            setPadding(padH, MantaTheme.dp(context, 22f), padH, MantaTheme.dp(context, 160f))
        }

        // 1. Top Header Bar (`MANTA` + Settings/CORS toggle button)
        val headerRow = LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
        }

        val brandTitle = TextView(context).apply {
            text = "M A N T A"
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 22f
            typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
        }
        headerRow.addView(brandTitle)

        val corsBadgeBtn = TextView(context).apply {
            text = "CORS: OPEN"
            setTextColor(MantaTheme.ACCENT_EMERALD)
            textSize = 11.5f
            typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            val pH = MantaTheme.dp(context, 12f)
            val pV = MantaTheme.dp(context, 7f)
            setPadding(pH, pV, pH, pV)
            background = MantaTheme.roundedBg(
                fillColor = MantaTheme.BG_SURFACE,
                radiusDp = 14f,
                context = context,
                strokeColor = MantaTheme.BORDER_SUBTLE,
                strokeWidthDp = 1f
            )
            setOnClickListener {
                corsEnabled = !corsEnabled
                text = if (corsEnabled) "CORS: OPEN" else "CORS: STRICT"
                setTextColor(if (corsEnabled) MantaTheme.ACCENT_EMERALD else MantaTheme.TEXT_SECONDARY)
                onToggleCorsDev(corsEnabled)
            }
        }
        headerRow.addView(corsBadgeBtn)
        root.addView(headerRow)

        // 2. Horizontal Category Chips Row (Muse-style)
        val chipsScroll = HorizontalScrollView(context).apply {
            isHorizontalScrollBarEnabled = false
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                topMargin = MantaTheme.dp(context, 18f)
                bottomMargin = MantaTheme.dp(context, 22f)
            }
        }
        val chipsRow = LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
        }
        val categories = listOf(
            "All Curated",
            "Rust Stream",
            "CORS Dev Test",
            "Zero-Cookie Shield",
            "Instant Wipe"
        )
        categories.forEachIndexed { idx, label ->
            val chip = TextView(context).apply {
                text = label
                textSize = 13.5f
                typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
                val pH = MantaTheme.dp(context, 18f)
                val pV = MantaTheme.dp(context, 9f)
                setPadding(pH, pV, pH, pV)
                layoutParams = LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.WRAP_CONTENT,
                    LinearLayout.LayoutParams.WRAP_CONTENT
                ).apply {
                    marginEnd = MantaTheme.dp(context, 10f)
                }
                setOnClickListener {
                    if (idx == 4) {
                        onTriggerZeroize()
                    } else {
                        selectCategoryChip(idx)
                    }
                }
            }
            categoryChipViews.add(chip)
            chipsRow.addView(chip)
        }
        chipsScroll.addView(chipsRow)
        root.addView(chipsScroll)
        selectCategoryChip(0)

        // 3. Section: FEATURED ENGINE WORKSPACES
        root.addView(makeOverline(context, "RUST NATIVE ENGINE"))
        root.addView(makeSectionTitle(context, "Developer & Zero-Trace Workspaces"))

        val featuredScroll = HorizontalScrollView(context).apply {
            isHorizontalScrollBarEnabled = false
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                topMargin = MantaTheme.dp(context, 12f)
                bottomMargin = MantaTheme.dp(context, 26f)
            }
        }
        val featuredRow = LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
        }

        val featuredCards = listOf(
            QuickLaunchSite(
                title = "Manta Rust Benchmark",
                subtitle = "12 KB RAM • Direct Rust DOM",
                url = "manta://benchmark",
                accentColor = 0xFF1E3A2F.toInt(),
                badge = "RUST"
            ),
            QuickLaunchSite(
                title = "CORS API Sandbox",
                subtitle = "Preflight 204 • Open Headers",
                url = "manta://cors-lab",
                accentColor = 0xFF2E1F47.toInt(),
                badge = "CORS"
            ),
            QuickLaunchSite(
                title = "Localhost Dev Runner",
                subtitle = "http://localhost:3000",
                url = "http://localhost:3000",
                accentColor = 0xFF172554.toInt(),
                badge = "DEV"
            )
        )

        for (item in featuredCards) {
            featuredRow.addView(makeFeaturedCard(context, item))
        }
        featuredScroll.addView(featuredRow)
        root.addView(featuredScroll)

        // 4. Section: SPEED DIAL & WEB LAUNCHPAD
        root.addView(makeSectionTitle(context, "Fast Stream Launchpad"))
        root.addView(makeSubtitleText(context, "Zero cookies saved • Automatic RAM wipe on close"))

        val speedDialSites = listOf(
            QuickLaunchSite(
                title = "DuckDuckGo Lite",
                subtitle = "Zero-tracking instant web search",
                url = "https://lite.duckduckgo.com/lite/",
                accentColor = 0xFF1E293B.toInt(),
                badge = "SEARCH"
            ),
            QuickLaunchSite(
                title = "Hacker News",
                subtitle = "news.ycombinator.com • Ultra-fast load",
                url = "https://news.ycombinator.com",
                accentColor = 0xFF27272A.toInt(),
                badge = "TECH"
            ),
            QuickLaunchSite(
                title = "MDN Web Docs",
                subtitle = "developer.mozilla.org • Web API reference",
                url = "https://developer.mozilla.org/en-US/",
                accentColor = 0xFF1E1B4B.toInt(),
                badge = "DOCS"
            ),
            QuickLaunchSite(
                title = "GitHub Mobile",
                subtitle = "github.com • Repositories & Actions",
                url = "https://github.com",
                accentColor = 0xFF18181B.toInt(),
                badge = "CODE"
            ),
            QuickLaunchSite(
                title = "HTTPBin Inspector",
                subtitle = "httpbin.org/headers • Verify zero-cookie headers",
                url = "https://httpbin.org/headers",
                accentColor = 0xFF0F2922.toInt(),
                badge = "TEST"
            )
        )

        for (site in speedDialSites) {
            root.addView(makeSpeedDialRowCard(context, site))
        }

        addView(root)
    }

    private fun selectCategoryChip(index: Int) {
        selectedCategoryIndex = index
        categoryChipViews.forEachIndexed { i, tv ->
            val active = i == index
            tv.setTextColor(if (active) MantaTheme.BG_OBSIDIAN else MantaTheme.TEXT_SECONDARY)
            tv.background = MantaTheme.roundedBg(
                fillColor = if (active) MantaTheme.BG_CHIP_ACTIVE else MantaTheme.BG_CHIP_INACTIVE,
                radiusDp = 20f,
                context = context,
                strokeColor = if (active) 0 else MantaTheme.BORDER_SUBTLE,
                strokeWidthDp = if (active) 0f else 1f
            )
        }
    }

    private fun makeOverline(context: Context, text: String): TextView {
        return TextView(context).apply {
            this.text = text
            setTextColor(MantaTheme.TEXT_MUTED)
            textSize = 11f
            typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            letterSpacing = 0.12f
        }
    }

    private fun makeSectionTitle(context: Context, text: String): TextView {
        return TextView(context).apply {
            this.text = text
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 20f
            typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                topMargin = MantaTheme.dp(context, 3f)
            }
        }
    }

    private fun makeSubtitleText(context: Context, text: String): TextView {
        return TextView(context).apply {
            this.text = text
            setTextColor(MantaTheme.TEXT_SECONDARY)
            textSize = 12.5f
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                topMargin = MantaTheme.dp(context, 2f)
                bottomMargin = MantaTheme.dp(context, 12f)
            }
        }
    }

    private fun makeFeaturedCard(context: Context, item: QuickLaunchSite): LinearLayout {
        val cardWidth = MantaTheme.dp(context, 168f)
        val cardHeight = MantaTheme.dp(context, 168f)

        val wrapper = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            layoutParams = LinearLayout.LayoutParams(cardWidth, LinearLayout.LayoutParams.WRAP_CONTENT).apply {
                marginEnd = MantaTheme.dp(context, 14f)
            }
            setOnClickListener {
                onLaunchUrl(item.url, item.url.startsWith("manta://"))
            }
        }

        val artworkBox = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.BOTTOM
            val p = MantaTheme.dp(context, 14f)
            setPadding(p, p, p, p)
            layoutParams = LinearLayout.LayoutParams(cardWidth, cardHeight)
            background = MantaTheme.roundedBg(
                fillColor = item.accentColor,
                radiusDp = 18f,
                context = context,
                strokeColor = MantaTheme.BORDER_SUBTLE,
                strokeWidthDp = 1f
            )
        }

        val badgeTv = TextView(context).apply {
            text = item.badge
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 11f
            typeface = Typeface.create(Typeface.MONOSPACE, Typeface.BOLD)
        }
        val centerLabel = TextView(context).apply {
            text = item.title
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 18f
            typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                0,
                1f
            ).apply {
                topMargin = MantaTheme.dp(context, 14f)
            }
        }
        val playBadge = TextView(context).apply {
            text = "▶"
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 13f
            gravity = Gravity.CENTER
            background = MantaTheme.roundedBg(0xFF09090B.toInt(), 10f, context)
            val s = MantaTheme.dp(context, 34f)
            layoutParams = LinearLayout.LayoutParams(s, s).apply {
                gravity = Gravity.END
            }
        }

        artworkBox.addView(badgeTv)
        artworkBox.addView(centerLabel)
        artworkBox.addView(playBadge)
        wrapper.addView(artworkBox)

        val titleTv = TextView(context).apply {
            text = item.title
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 14f
            typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                topMargin = MantaTheme.dp(context, 8f)
            }
        }
        val subTv = TextView(context).apply {
            text = item.subtitle
            setTextColor(MantaTheme.TEXT_SECONDARY)
            textSize = 12f
        }
        wrapper.addView(titleTv)
        wrapper.addView(subTv)

        return wrapper
    }

    private fun makeSpeedDialRowCard(context: Context, site: QuickLaunchSite): LinearLayout {
        return LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            val p = MantaTheme.dp(context, 14f)
            setPadding(p, p, p, p)
            background = MantaTheme.roundedBg(
                fillColor = MantaTheme.BG_CARD,
                radiusDp = 16f,
                context = context,
                strokeColor = MantaTheme.BORDER_SUBTLE,
                strokeWidthDp = 1f
            )
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                bottomMargin = MantaTheme.dp(context, 10f)
            }
            setOnClickListener {
                onLaunchUrl(site.url, false)
            }

            val badge = TextView(context).apply {
                text = site.badge
                setTextColor(MantaTheme.ACCENT_CYAN)
                textSize = 10.5f
                typeface = Typeface.create(Typeface.MONOSPACE, Typeface.BOLD)
                gravity = Gravity.CENTER
                background = MantaTheme.roundedBg(site.accentColor, 10f, context)
                layoutParams = LinearLayout.LayoutParams(
                    MantaTheme.dp(context, 56f),
                    MantaTheme.dp(context, 42f)
                )
            }
            addView(badge)

            val col = LinearLayout(context).apply {
                orientation = LinearLayout.VERTICAL
                layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f).apply {
                    marginStart = MantaTheme.dp(context, 12f)
                }
            }
            col.addView(TextView(context).apply {
                text = site.title
                setTextColor(MantaTheme.TEXT_PRIMARY)
                textSize = 14.5f
                typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            })
            col.addView(TextView(context).apply {
                text = site.subtitle
                setTextColor(MantaTheme.TEXT_SECONDARY)
                textSize = 12f
            })
            addView(col)

            addView(TextView(context).apply {
                text = "→"
                setTextColor(MantaTheme.TEXT_SECONDARY)
                textSize = 16f
            })
        }
    }
}
