package app.manta.browser.ui.screens

import android.content.Context
import android.graphics.Typeface
import android.view.Gravity
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import app.manta.browser.core.MantaNative
import app.manta.browser.ui.theme.MantaTheme

data class EphemeralTabInfo(
    val id: Int,
    var title: String,
    var url: String
)

/**
 * Library & Ephemeral Sandbox Screen (Dock Section 2):
 * - Manage isolated Rust Tab Sandboxes (background tabs auto-hibernate in < 1 KB RAM)
 * - Inspect or zeroize the Rust `ClipboardSandbox`
 * - View live Anti-Fingerprinting & CORS Dev logs
 */
class LibraryTabsScreen(
    context: Context,
    private val tabs: MutableList<EphemeralTabInfo>,
    private val getActiveTabId: () -> Int,
    private val onSelectTab: (Int) -> Unit,
    private val onCreateNewTab: () -> Unit,
    private val onCloseTab: (Int) -> Unit,
    private val onZeroizeAll: () -> Unit
) : ScrollView(context) {

    private val tabsListContainer: LinearLayout
    private val clipboardPreview: TextView
    private val shieldLogContainer: LinearLayout
    private val ramStatsText: TextView

    init {
        setBackgroundColor(MantaTheme.BG_OBSIDIAN)
        isVerticalScrollBarEnabled = false

        val root = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            val padH = MantaTheme.dp(context, 18f)
            setPadding(padH, MantaTheme.dp(context, 22f), padH, MantaTheme.dp(context, 160f))
        }

        // Header + "+ New Tab" Button
        val header = LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
        }
        header.addView(TextView(context).apply {
            text = "Ephemeral Library"
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 22f
            typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
        })

        val newTabBtn = TextView(context).apply {
            text = "+ New Tab"
            setTextColor(MantaTheme.BG_OBSIDIAN)
            textSize = 12.5f
            typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            val pH = MantaTheme.dp(context, 14f)
            val pV = MantaTheme.dp(context, 8f)
            setPadding(pH, pV, pH, pV)
            background = MantaTheme.roundedBg(MantaTheme.TEXT_PRIMARY, 16f, context)
            setOnClickListener { onCreateNewTab() }
        }
        header.addView(newTabBtn)
        root.addView(header)

        // Live Memory Telemetry Card
        val memCard = LinearLayout(context).apply {
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
                topMargin = MantaTheme.dp(context, 16f)
                bottomMargin = MantaTheme.dp(context, 20f)
            }
        }

        ramStatsText = TextView(context).apply {
            text = "Volatile Arena: 0.0 KB • Disk Cache: 0 Bytes"
            setTextColor(MantaTheme.ACCENT_EMERALD)
            textSize = 13f
            typeface = Typeface.create(Typeface.MONOSPACE, Typeface.BOLD)
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
        }
        memCard.addView(ramStatsText)

        val zeroizeBtn = TextView(context).apply {
            text = "Wipe All RAM"
            setTextColor(MantaTheme.ACCENT_ROSE)
            textSize = 12f
            typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            val pH = MantaTheme.dp(context, 12f)
            val pV = MantaTheme.dp(context, 7f)
            setPadding(pH, pV, pH, pV)
            background = MantaTheme.roundedBg(0xFF2A1219.toInt(), 12f, context)
            setOnClickListener { onZeroizeAll() }
        }
        memCard.addView(zeroizeBtn)
        root.addView(memCard)

        // Active & Hibernated Tabs List
        root.addView(makeSectionLabel(context, "ACTIVE & HIBERNATED TABS"))
        tabsListContainer = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                topMargin = MantaTheme.dp(context, 8f)
                bottomMargin = MantaTheme.dp(context, 22f)
            }
        }
        root.addView(tabsListContainer)

        // Sandboxed Rust Clipboard Vault
        root.addView(makeSectionLabel(context, "RUST SANDBOXED CLIPBOARD"))
        val clipCard = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
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
                topMargin = MantaTheme.dp(context, 8f)
                bottomMargin = MantaTheme.dp(context, 22f)
            }
        }
        clipboardPreview = TextView(context).apply {
            text = "(Clipboard Vault Empty)"
            setTextColor(MantaTheme.TEXT_SECONDARY)
            textSize = 12.5f
            typeface = Typeface.MONOSPACE
        }
        clipCard.addView(clipboardPreview)
        root.addView(clipCard)

        // Shield & CORS Intercept Log
        root.addView(makeSectionLabel(context, "SHIELD & CORS STREAM LOG"))
        shieldLogContainer = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                topMargin = MantaTheme.dp(context, 8f)
            }
        }
        root.addView(shieldLogContainer)

        addView(root)
    }

    fun refreshLibraryState(logs: List<String>) {
        val ramBytes = MantaNative.getEngineRamBytes()
        ramStatsText.text = String.format(
            "Volatile Arena: %.1f KB • Disk: 0 B",
            ramBytes / 1024f
        )

        val clip = MantaNative.getClipboardText()
        clipboardPreview.text = if (clip.isBlank()) "(Clipboard Vault Empty)" else clip

        tabsListContainer.removeAllViews()
        val activeId = getActiveTabId()
        for (tab in tabs) {
            val isActive = tab.id == activeId
            val row = LinearLayout(context).apply {
                orientation = LinearLayout.HORIZONTAL
                gravity = Gravity.CENTER_VERTICAL
                val p = MantaTheme.dp(context, 13f)
                setPadding(p, p, p, p)
                background = MantaTheme.roundedBg(
                    fillColor = if (isActive) MantaTheme.BG_DOCK_PILL else MantaTheme.BG_CARD,
                    radiusDp = 14f,
                    context = context,
                    strokeColor = if (isActive) MantaTheme.ACCENT_CYAN else MantaTheme.BORDER_SUBTLE,
                    strokeWidthDp = 1f
                )
                layoutParams = LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.MATCH_PARENT,
                    LinearLayout.LayoutParams.WRAP_CONTENT
                ).apply {
                    bottomMargin = MantaTheme.dp(context, 8f)
                }
                setOnClickListener { onSelectTab(tab.id) }
            }

            val col = LinearLayout(context).apply {
                orientation = LinearLayout.VERTICAL
                layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
            }
            col.addView(TextView(context).apply {
                text = "${tab.title} ${if (isActive) "• ACTIVE" else "• HIBERNATED"}"
                setTextColor(MantaTheme.TEXT_PRIMARY)
                textSize = 13.5f
                typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            })
            col.addView(TextView(context).apply {
                text = tab.url
                setTextColor(MantaTheme.TEXT_SECONDARY)
                textSize = 11.5f
            })
            row.addView(col)

            val closeBtn = TextView(context).apply {
                text = "✕"
                setTextColor(MantaTheme.TEXT_SECONDARY)
                textSize = 14f
                val pad = MantaTheme.dp(context, 8f)
                setPadding(pad, pad, pad, pad)
                setOnClickListener { onCloseTab(tab.id) }
            }
            row.addView(closeBtn)
            tabsListContainer.addView(row)
        }

        shieldLogContainer.removeAllViews()
        if (logs.isEmpty()) {
            shieldLogContainer.addView(TextView(context).apply {
                text = "No trackers encountered yet • Zero-Cookie Shield active"
                setTextColor(MantaTheme.TEXT_MUTED)
                textSize = 12f
            })
        } else {
            for (entry in logs.takeLast(8).reversed()) {
                shieldLogContainer.addView(TextView(context).apply {
                    text = "• $entry"
                    setTextColor(MantaTheme.ACCENT_CYAN)
                    textSize = 12f
                    typeface = Typeface.MONOSPACE
                    setPadding(0, MantaTheme.dp(context, 3f), 0, MantaTheme.dp(context, 3f))
                })
            }
        }
    }

    private fun makeSectionLabel(context: Context, text: String): TextView {
        return TextView(context).apply {
            this.text = text
            setTextColor(MantaTheme.TEXT_MUTED)
            textSize = 11f
            typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            letterSpacing = 0.1f
        }
    }
}
