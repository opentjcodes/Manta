package app.manta.browser.ui.components

import android.content.Context
import android.graphics.Typeface
import android.text.TextUtils
import android.view.Gravity
import android.widget.LinearLayout
import android.widget.TextView
import app.manta.browser.ui.theme.MantaTheme

/**
 * Muse-inspired floating mini bar right above the bottom dock.
 * Shows active site title, live Rust RAM usage (in KB), CORS status,
 * quick jump to Search/Browser, and 1-tap Volatile RAM Zeroize.
 */
class MiniStatusPill(
    context: Context,
    onOpenActivePage: () -> Unit,
    onReloadPage: () -> Unit,
    onZeroizeRam: () -> Unit
) : LinearLayout(context) {

    private val badgeView: TextView
    private val titleView: TextView
    private val subtitleView: TextView

    init {
        orientation = HORIZONTAL
        gravity = Gravity.CENTER_VERTICAL
        val padH = MantaTheme.dp(context, 12f)
        val padV = MantaTheme.dp(context, 10f)
        setPadding(padH, padV, padH, padV)
        background = MantaTheme.roundedBg(
            fillColor = MantaTheme.BG_SURFACE,
            radiusDp = 18f,
            context = context,
            strokeColor = MantaTheme.BORDER_SUBTLE,
            strokeWidthDp = 1f
        )

        // Left Monogram / Shield Badge
        badgeView = TextView(context).apply {
            text = "M"
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 14f
            typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            gravity = Gravity.CENTER
            background = MantaTheme.roundedBg(0xFF1E293B.toInt(), 11f, context)
            layoutParams = LayoutParams(
                MantaTheme.dp(context, 38f),
                MantaTheme.dp(context, 38f)
            )
        }
        addView(badgeView)

        // Center Title + Live Rust Telemetry
        val textCol = LinearLayout(context).apply {
            orientation = VERTICAL
            val lp = LayoutParams(0, LayoutParams.WRAP_CONTENT, 1f)
            lp.marginStart = MantaTheme.dp(context, 11f)
            lp.marginEnd = MantaTheme.dp(context, 8f)
            layoutParams = lp
            setOnClickListener { onOpenActivePage() }
        }

        titleView = TextView(context).apply {
            text = "Manta Rust Core Ready"
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 13.5f
            typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
            maxLines = 1
            ellipsize = TextUtils.TruncateAt.END
        }
        subtitleView = TextView(context).apply {
            text = "0.0 KB RAM • Zero Cache • CORS Open"
            setTextColor(MantaTheme.TEXT_SECONDARY)
            textSize = 11.5f
            maxLines = 1
            ellipsize = TextUtils.TruncateAt.END
        }
        textCol.addView(titleView)
        textCol.addView(subtitleView)
        addView(textCol)

        // Reload Button (styled like Muse's circular play button)
        val reloadBtn = TextView(context).apply {
            text = "↻"
            setTextColor(MantaTheme.TEXT_PRIMARY)
            textSize = 16f
            gravity = Gravity.CENTER
            background = MantaTheme.roundedBg(MantaTheme.BG_DOCK_PILL, 18f, context)
            val size = MantaTheme.dp(context, 34f)
            layoutParams = LayoutParams(size, size).apply {
                marginEnd = MantaTheme.dp(context, 6f)
            }
            setOnClickListener { onReloadPage() }
        }
        addView(reloadBtn)

        // 1-Tap Volatile RAM Zeroize Button
        val wipeBtn = TextView(context).apply {
            text = "⚡"
            setTextColor(MantaTheme.ACCENT_EMERALD)
            textSize = 14f
            gravity = Gravity.CENTER
            background = MantaTheme.roundedBg(0xFF132A24.toInt(), 18f, context)
            val size = MantaTheme.dp(context, 34f)
            layoutParams = LayoutParams(size, size)
            setOnClickListener { onZeroizeRam() }
        }
        addView(wipeBtn)
    }

    fun updateStatus(title: String, ramBytes: Int, corsEnabled: Boolean, tabCount: Int) {
        titleView.text = title.ifBlank { "Manta Rust Core" }
        val kb = ramBytes / 1024f
        val corsLabel = if (corsEnabled) "CORS Bypass ON" else "CORS Standard"
        subtitleView.text = String.format(
            "%.1f KB RAM • %d Tab%s • %s",
            kb,
            tabCount,
            if (tabCount == 1) "" else "s",
            corsLabel
        )
    }
}
