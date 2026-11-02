package app.manta.browser.ui.screens

import android.content.Context
import android.graphics.Typeface
import android.view.Gravity
import android.widget.GridLayout
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import app.manta.browser.core.MantaNative
import app.manta.browser.ui.theme.MantaTheme

data class QuickLaunchSite(
    val title: String,
    val subtitle: String,
    val url: String,
    val accentColor: Int,
    val badge: String
)

/**
 * Minimalist Native Browser Start Page (`HomeScreen.kt`):
 * - Displayed only on a fresh blank tab (`manta://start`).
 * - Borderless M3 Expressive aesthetics: clean Google Search trigger pill,
 *   8 minimalist squircle speed-dials, and a subtle Rust Core status pill.
 */
class HomeScreen(
    context: Context,
    private val onOpenOmniboxSearch: () -> Unit,
    private val onLaunchUrl: (url: String, preferRustReader: Boolean) -> Unit
) : ScrollView(context) {

    private val telemetryPill: TextView

    init {
        setBackgroundColor(MantaTheme.BG_OBSIDIAN)
        isVerticalScrollBarEnabled = false
        isFillViewport = true

        val container = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
            val padH = MantaTheme.dp(context, 24f)
            setPadding(padH, MantaTheme.dp(context, 84f), padH, MantaTheme.dp(context, 120f))
        }

        // 1. Minimalist Wordmark
        val logoBadge = TextView(context).apply {
            text = "manta"
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 36f
            typeface = Typeface.create("sans-serif-light", Typeface.NORMAL)
            letterSpacing = 0.08f
            gravity = Gravity.CENTER
        }
        container.addView(logoBadge)

        telemetryPill = TextView(context).apply {
            val cores = MantaNative.getWorkerCoreCount()
            text = "$cores Rust Cores • Zero Cookie Tracking • CORS Unlocked"
            setTextColor(MantaTheme.TEXT_MUTED)
            textSize = 12f
            gravity = Gravity.CENTER
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                topMargin = MantaTheme.dp(context, 6f)
                bottomMargin = MantaTheme.dp(context, 34f)
            }
        }
        container.addView(telemetryPill)

        // 2. Expressive Pill Search Bar Trigger (Defaults to Google Search)
        val searchPill = LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            val pH = MantaTheme.dp(context, 20f)
            val pV = MantaTheme.dp(context, 16f)
            setPadding(pH, pV, pH, pV)
            background = MantaTheme.roundedBg(MantaTheme.BG_CARD, 32f, context)
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                bottomMargin = MantaTheme.dp(context, 36f)
            }
            setOnClickListener { onOpenOmniboxSearch() }
        }

        val gIcon = TextView(context).apply {
            text = "G"
            setTextColor(MantaTheme.ACCENT_PRIMARY)
            textSize = 18f
            typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                marginEnd = MantaTheme.dp(context, 14f)
            }
        }
        searchPill.addView(gIcon)

        val promptTv = TextView(context).apply {
            text = "Search Google or type URL"
            setTextColor(MantaTheme.TEXT_SECONDARY)
            textSize = 15.5f
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
        }
        searchPill.addView(promptTv)
        container.addView(searchPill)

        // 3. Minimalist 4-Column Squircle Speed Dials (Zero lines, pure tonal squircles)
        val grid = GridLayout(context).apply {
            columnCount = 4
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            )
        }

        val dials = listOf(
            QuickLaunchSite("Google", "", "https://www.google.com", MantaTheme.CONTAINER_PRIMARY, "G"),
            QuickLaunchSite("GitHub", "", "https://github.com", MantaTheme.BG_CARD, "GH"),
            QuickLaunchSite("YouTube", "", "https://m.youtube.com", MantaTheme.CONTAINER_TERTIARY, "▶"),
            QuickLaunchSite("DuckDuckGo", "", "https://duckduckgo.com", MantaTheme.CONTAINER_SECONDARY, "DDG"),
            QuickLaunchSite("HackerNews", "", "https://news.ycombinator.com", MantaTheme.BG_CARD, "Y"),
            QuickLaunchSite("Wikipedia", "", "https://www.wikipedia.org", MantaTheme.BG_CARD, "W"),
            QuickLaunchSite("Localhost", "", "http://localhost:3000", MantaTheme.CONTAINER_PRIMARY, "DEV"),
            QuickLaunchSite("CORS Lab", "", "manta://cors-lab", MantaTheme.CONTAINER_SECONDARY, "LAB")
        )

        for (dial in dials) {
            val itemLayout = LinearLayout(context).apply {
                orientation = LinearLayout.VERTICAL
                gravity = Gravity.CENTER_HORIZONTAL
                val pad = MantaTheme.dp(context, 8f)
                setPadding(pad, pad, pad, MantaTheme.dp(context, 16f))
                layoutParams = GridLayout.LayoutParams().apply {
                    width = 0
                    columnSpec = GridLayout.spec(GridLayout.UNDEFINED, 1f)
                }
                setOnClickListener {
                    onLaunchUrl(dial.url, false)
                }
            }

            val squircle = TextView(context).apply {
                text = dial.badge
                setTextColor(MantaTheme.TEXT_PRIMARY)
                textSize = 14f
                typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
                gravity = Gravity.CENTER
                background = MantaTheme.roundedBg(dial.accentColor, 20f, context)
                val s = MantaTheme.dp(context, 56f)
                layoutParams = LinearLayout.LayoutParams(s, s)
            }
            itemLayout.addView(squircle)

            val label = TextView(context).apply {
                text = dial.title
                setTextColor(MantaTheme.TEXT_SECONDARY)
                textSize = 11.5f
                gravity = Gravity.CENTER
                maxLines = 1
                layoutParams = LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.WRAP_CONTENT,
                    LinearLayout.LayoutParams.WRAP_CONTENT
                ).apply {
                    topMargin = MantaTheme.dp(context, 7f)
                }
            }
            itemLayout.addView(label)
            grid.addView(itemLayout)
        }

        container.addView(grid)
        addView(container)
    }
}
