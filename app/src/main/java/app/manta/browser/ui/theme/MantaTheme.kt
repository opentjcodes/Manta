package app.manta.browser.ui.theme

import android.content.Context
import android.graphics.drawable.GradientDrawable
import android.util.TypedValue

/**
 * Material 3 Expressive × Minimalist Tonal Design System:
 * - Zero borders / zero 1px lines anywhere in the UI.
 * - Pure tonal surface hierarchy with expressive squircle/pill radii.
 */
object MantaTheme {
    // M3 Dark Tonal Surfaces (No borders needed)
    const val BG_OBSIDIAN = 0xFF090A0F.toInt()
    const val BG_SURFACE = 0xFF12141C.toInt()
    const val BG_CARD = 0xFF191C26.toInt()
    const val BG_CONTAINER_HIGH = 0xFF232734.toInt()
    const val BG_DOCK = 0xF2161922.toInt()
    const val BG_DOCK_PILL = 0xFF252A38.toInt()
    const val BG_CHIP_INACTIVE = 0xFF191C26.toInt()
    const val BG_CHIP_ACTIVE = 0xFFA8C7FA.toInt()

    // Expressive Tonal Containers
    const val CONTAINER_PRIMARY = 0xFF1E3252.toInt()
    const val ON_CONTAINER_PRIMARY = 0xFFD3E3FD.toInt()
    const val CONTAINER_SECONDARY = 0xFF15332B.toInt()
    const val ON_CONTAINER_SECONDARY = 0xFF6DD58C.toInt()
    const val CONTAINER_TERTIARY = 0xFF3B1922.toInt()
    const val ON_CONTAINER_TERTIARY = 0xFFFFB3B8.toInt()

    // Kept as 0 (transparent) so legacy calls never draw borders
    const val BORDER_SUBTLE = 0x00000000

    // Typography Colors
    const val TEXT_PRIMARY = 0xFFF1F5F9.toInt()
    const val TEXT_SECONDARY = 0xFF94A3B8.toInt()
    const val TEXT_MUTED = 0xFF64748B.toInt()

    // M3 Expressive Accents
    const val ACCENT_PRIMARY = 0xFFA8C7FA.toInt()
    const val ACCENT_EMERALD = 0xFF6DD58C.toInt()
    const val ACCENT_CYAN = 0xFF7FCFFF.toInt()
    const val ACCENT_ROSE = 0xFFFF897D.toInt()

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

    /**
     * Borderless M3 Expressive Squircle / Pill background generator.
     */
    fun roundedBg(
        fillColor: Int,
        radiusDp: Float,
        context: Context,
        @Suppress("UNUSED_PARAMETER") strokeColor: Int = 0,
        @Suppress("UNUSED_PARAMETER") strokeWidthDp: Float = 0f
    ): GradientDrawable {
        return GradientDrawable().apply {
            shape = GradientDrawable.RECTANGLE
            cornerRadius = dpF(context, radiusDp)
            setColor(fillColor)
        }
    }

    fun extractCleanHost(url: String): String {
        if (url.isBlank() || url == "manta://start") return "Search Google or type URL"
        if (url.startsWith("manta://")) return url
        return try {
            val withoutScheme = url.substringAfter("://")
            val host = withoutScheme.substringBefore('/').substringBefore('?')
            host.removePrefix("www.")
        } catch (_: Throwable) {
            url
        }
    }
}
