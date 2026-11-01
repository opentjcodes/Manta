package app.manta.browser.ui.theme

import android.content.Context
import android.graphics.drawable.GradientDrawable
import android.util.TypedValue

/**
 * Minimalist Pure Obsidian Dark Design Language inspired by Muse.
 */
object MantaTheme {
    const val BG_OBSIDIAN = 0xFF000000.toInt()
    const val BG_SURFACE = 0xFF111318.toInt()
    const val BG_CARD = 0xFF161920.toInt()
    const val BG_DOCK = 0xFF12141A.toInt()
    const val BG_DOCK_PILL = 0xFF242832.toInt()
    const val BG_CHIP_INACTIVE = 0xFF171A21.toInt()
    const val BG_CHIP_ACTIVE = 0xFFFFFFFF.toInt()

    const val BORDER_SUBTLE = 0xFF222631.toInt()
    const val TEXT_PRIMARY = 0xFFFFFFFF.toInt()
    const val TEXT_SECONDARY = 0xFF94A3B8.toInt()
    const val TEXT_MUTED = 0xFF64748B.toInt()

    const val ACCENT_EMERALD = 0xFF10B981.toInt()
    const val ACCENT_CYAN = 0xFF38BDF8.toInt()
    const val ACCENT_INDIGO = 0xFF6366F1.toInt()
    const val ACCENT_ROSE = 0xFFF43F5E.toInt()

    fun dp(context: Context, value: Float): Int {
        return TypedValue.applyDimension(
            TypedValue.COMPLEX_UNIT_DIP,
            value,
            context.resources.displayMetrics
        ).toInt()
    }

    fun dpF(context: Context, value: Float): Float {
        return TypedValue.applyDimension(
            TypedValue.COMPLEX_UNIT_DIP,
            value,
            context.resources.displayMetrics
        )
    }

    fun roundedBg(
        fillColor: Int,
        radiusDp: Float,
        context: Context,
        strokeColor: Int = 0,
        strokeWidthDp: Float = 0f
    ): GradientDrawable {
        return GradientDrawable().apply {
            shape = GradientDrawable.RECTANGLE
            cornerRadius = dpF(context, radiusDp)
            setColor(fillColor)
            if (strokeWidthDp > 0f && strokeColor != 0) {
                setStroke(dp(context, strokeWidthDp), strokeColor)
            }
        }
    }
}
