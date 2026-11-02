package app.manta.browser.ui.components

import android.content.Context
import android.graphics.Typeface
import android.view.Gravity
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.HorizontalScrollView
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.Toast
import app.manta.browser.core.MantaNative
import app.manta.browser.ui.theme.MantaTheme

/**
 * M3 Expressive Floating Omnibox & Engine Control Sheet (`MiniStatusPill.kt`):
 * - Appears as a borderless, glass-like M3 Expressive overlay when tapping the address bar or menu.
 * - Uses Google Search (`https://www.google.com/search?q=...`) as the default search engine.
 * - Provides 1-tap domain helpers, Reload, Reader Mode toggle, CORS Dev toggle, and Instant RAM Zeroize.
 */
class MiniStatusPill(
    context: Context,
    private val onNavigateToUrlOrSearch: (queryOrUrl: String, forceReader: Boolean) -> Unit,
    private val onReloadPage: () -> Unit,
    private val onToggleReaderMode: () -> Unit,
    private val onToggleCorsMode: (Boolean) -> Unit,
    private val onZeroizeRam: () -> Unit
) : FrameLayout(context) {

    private val addressInput: EditText
    private val telemetrySubtitle: TextView
    private val corsPillBtn: TextView
    private val readerPillBtn: TextView

    private var corsEnabled: Boolean = true
    private var readerEnabled: Boolean = false

    init {
        visibility = View.GONE
        setBackgroundColor(0xB805060A.toInt()) // Soft scrim backdrop
        setOnClickListener { dismissOverlay() }

        val sheetCard = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            val pad = MantaTheme.dp(context, 18f)
            setPadding(pad, pad, pad, pad)
            background = MantaTheme.roundedBg(MantaTheme.BG_SURFACE, 28f, context)
            val margin = MantaTheme.dp(context, 14f)
            layoutParams = LayoutParams(
                LayoutParams.MATCH_PARENT,
                LayoutParams.WRAP_CONTENT,
                Gravity.BOTTOM
            ).apply {
                setMargins(margin, margin, margin, MantaTheme.dp(context, 78f))
            }
            setOnClickListener { /* Consume clicks inside card */ }
        }

        // 1. Top Search / URL Input Pill (Borderless M3 Container High)
        val inputRow = LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            val pH = MantaTheme.dp(context, 14f)
            val pV = MantaTheme.dp(context, 6f)
            setPadding(pH, pV, pH, pV)
            background = MantaTheme.roundedBg(MantaTheme.BG_CONTAINER_HIGH, 22f, context)
        }

        val googleBadge = TextView(context).apply {
            text = "G"
            setTextColor(MantaTheme.ACCENT_PRIMARY)
            textSize = 15f
            typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                marginEnd = MantaTheme.dp(context, 10f)
            }
        }
        inputRow.addView(googleBadge)

        addressInput = EditText(context).apply {
            hint = "Search Google or type URL..."
            setHintTextColor(MantaTheme.TEXT_MUTED)
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 15f
            isSingleLine = true
            background = null
            imeOptions = EditorInfo.IME_ACTION_GO
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
            setOnEditorActionListener { _, actionId, event ->
                if (actionId == EditorInfo.IME_ACTION_GO ||
                    actionId == EditorInfo.IME_ACTION_SEARCH ||
                    (event != null && event.keyCode == KeyEvent.KEYCODE_ENTER)
                ) {
                    val query = text.toString().trim()
                    if (query.isNotEmpty()) {
                        dismissOverlay()
                        onNavigateToUrlOrSearch(query, false)
                    }
                    true
                } else {
                    false
                }
            }
        }
        inputRow.addView(addressInput)

        val clearBtn = TextView(context).apply {
            text = "✕"
            setTextColor(MantaTheme.TEXT_SECONDARY)
            textSize = 14f
            val p = MantaTheme.dp(context, 8f)
            setPadding(p, p, p, p)
            setOnClickListener { addressInput.setText("") }
        }
        inputRow.addView(clearBtn)
        sheetCard.addView(inputRow)

        // 2. Quick Domain & Search Tokens Row
        val tokensScroll = HorizontalScrollView(context).apply {
            isHorizontalScrollBarEnabled = false
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                topMargin = MantaTheme.dp(context, 10f)
                bottomMargin = MantaTheme.dp(context, 14f)
            }
        }
        val tokensRow = LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
        }
        val quickTokens = listOf("https://", ".com", ".dev", "localhost:3000", "google.com", "github.com")
        for (tok in quickTokens) {
            val chip = TextView(context).apply {
                text = tok
                setTextColor(MantaTheme.TEXT_SECONDARY)
                textSize = 12f
                val pH = MantaTheme.dp(context, 12f)
                val pV = MantaTheme.dp(context, 6f)
                setPadding(pH, pV, pH, pV)
                background = MantaTheme.roundedBg(MantaTheme.BG_CARD, 14f, context)
                layoutParams = LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.WRAP_CONTENT,
                    LinearLayout.LayoutParams.WRAP_CONTENT
                ).apply {
                    marginEnd = MantaTheme.dp(context, 6f)
                }
                setOnClickListener {
                    val cur = addressInput.text.toString()
                    addressInput.setText("$cur$tok")
                    addressInput.setSelection(addressInput.text.length)
                }
            }
            tokensRow.addView(chip)
        }
        tokensScroll.addView(tokensRow)
        sheetCard.addView(tokensScroll)

        // 3. Expressive Action Grid (Reload, Copy, Reader Mode, CORS Bypass, Zeroize)
        val actionsRow = LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            )
        }

        actionsRow.addView(makeExpressivePillButton(context, "↻ Reload", MantaTheme.BG_CARD, MantaTheme.TEXT_PRIMARY) {
            dismissOverlay()
            onReloadPage()
        })

        actionsRow.addView(makeExpressivePillButton(context, "Copy URL", MantaTheme.BG_CARD, MantaTheme.TEXT_PRIMARY) {
            MantaNative.copyToClipboard(addressInput.text.toString())
            Toast.makeText(context, "Copied URL", Toast.LENGTH_SHORT).show()
            dismissOverlay()
        })

        readerPillBtn = makeExpressivePillButton(
            context,
            "Reader: OFF",
            MantaTheme.BG_CARD,
            MantaTheme.TEXT_SECONDARY
        ) {
            readerEnabled = !readerEnabled
            updateToggleLabels()
            dismissOverlay()
            onToggleReaderMode()
        }
        actionsRow.addView(readerPillBtn)
        sheetCard.addView(actionsRow)

        val secondRow = LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                topMargin = MantaTheme.dp(context, 8f)
            }
        }

        corsPillBtn = makeExpressivePillButton(
            context,
            "CORS Bypass: ON",
            MantaTheme.CONTAINER_SECONDARY,
            MantaTheme.ON_CONTAINER_SECONDARY
        ) {
            corsEnabled = !corsEnabled
            updateToggleLabels()
            onToggleCorsMode(corsEnabled)
        }
        secondRow.addView(corsPillBtn)

        secondRow.addView(
            makeExpressivePillButton(
                context,
                "⚡ Zeroize RAM",
                MantaTheme.CONTAINER_TERTIARY,
                MantaTheme.ON_CONTAINER_TERTIARY
            ) {
                dismissOverlay()
                onZeroizeRam()
            }
        )
        sheetCard.addView(secondRow)

        // 4. Subtle Rust Engine Live Telemetry Footer
        telemetrySubtitle = TextView(context).apply {
            text = "Manta Multi-Core Rust Engine • 0 B Disk Cache"
            setTextColor(MantaTheme.TEXT_MUTED)
            textSize = 11.5f
            gravity = Gravity.CENTER
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                topMargin = MantaTheme.dp(context, 12f)
            }
        }
        sheetCard.addView(telemetrySubtitle)

        addView(sheetCard)
    }

    private fun makeExpressivePillButton(
        context: Context,
        label: String,
        bgColor: Int,
        textColor: Int,
        onClick: () -> Unit
    ): TextView {
        return TextView(context).apply {
            text = label
            setTextColor(textColor)
            textSize = 12.5f
            gravity = Gravity.CENTER
            typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
            val pV = MantaTheme.dp(context, 11f)
            setPadding(MantaTheme.dp(context, 8f), pV, MantaTheme.dp(context, 8f), pV)
            background = MantaTheme.roundedBg(bgColor, 18f, context)
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f).apply {
                marginEnd = MantaTheme.dp(context, 6f)
            }
            setOnClickListener { onClick() }
        }
    }

    private fun updateToggleLabels() {
        corsPillBtn.text = if (corsEnabled) "CORS Bypass: ON" else "CORS: Standard"
        corsPillBtn.background = MantaTheme.roundedBg(
            if (corsEnabled) MantaTheme.CONTAINER_SECONDARY else MantaTheme.BG_CARD,
            18f,
            context
        )
        corsPillBtn.setTextColor(
            if (corsEnabled) MantaTheme.ON_CONTAINER_SECONDARY else MantaTheme.TEXT_SECONDARY
        )

        readerPillBtn.text = if (readerEnabled) "Reader: ON" else "Reader: OFF"
        readerPillBtn.background = MantaTheme.roundedBg(
            if (readerEnabled) MantaTheme.CONTAINER_PRIMARY else MantaTheme.BG_CARD,
            18f,
            context
        )
        readerPillBtn.setTextColor(
            if (readerEnabled) MantaTheme.ON_CONTAINER_PRIMARY else MantaTheme.TEXT_SECONDARY
        )
    }

    fun showOmnibox(currentUrl: String, focusKeyboard: Boolean) {
        val editableUrl = if (currentUrl == "manta://start") "" else currentUrl
        addressInput.setText(editableUrl)
        visibility = View.VISIBLE
        if (focusKeyboard) {
            addressInput.requestFocus()
            addressInput.selectAll()
            postDelayed({
                val imm = context.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager
                imm?.showSoftInput(addressInput, InputMethodManager.SHOW_IMPLICIT)
            }, 60L)
        }
    }

    fun dismissOverlay() {
        try {
            val imm = context.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager
            imm?.hideSoftInputFromWindow(addressInput.windowToken, 0)
            addressInput.clearFocus()
        } catch (_: Throwable) {
        }
        visibility = View.GONE
    }

    fun updateStatus(title: String, ramBytes: Int, isCorsOn: Boolean, tabCount: Int) {
        corsEnabled = isCorsOn
        updateToggleLabels()
        val cores = MantaNative.getWorkerCoreCount()
        val kb = ramBytes / 1024f
        telemetrySubtitle.text = String.format(
            "%s • %.1f KB RAM • %d Rust Cores • %d Tab%s",
            title.take(24),
            kb,
            cores,
            tabCount,
            if (tabCount == 1) "" else "s"
        )
    }
}
