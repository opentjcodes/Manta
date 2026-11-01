package app.manta.browser.ui.components

import android.animation.ValueAnimator
import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RectF
import android.graphics.Typeface
import android.view.MotionEvent
import android.view.View
import android.view.animation.DecelerateInterpolator
import app.manta.browser.ui.theme.MantaTheme
import kotlin.math.abs

/**
 * Muse-Inspired Interactive Sliding Bottom Dock:
 * - 3 Sections: Home (0) | Search (1) | Library (2)
 * - Supports both smooth finger dragging (sliding the highlight pill directly across tabs)
 *   and tap-to-animate transitions with zero GC churn.
 */
class SlidingBottomDock(
    context: Context,
    private val onSectionSelected: (Int) -> Unit
) : View(context) {

    private val items = arrayOf("Home", "Search", "Library")
    private var selectedIndex: Int = 0

    // Fractional position in [0f, 2f] representing the current center of the sliding pill
    private var pillPosition: Float = 0f
    private var isDraggingPill: Boolean = false
    private var downX: Float = 0f
    private var downY: Float = 0f
    private var animator: ValueAnimator? = null

    private val outerRect = RectF()
    private val pillRect = RectF()
    private val iconPath = Path()

    private val dockBgPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = MantaTheme.BG_DOCK
        style = Paint.Style.FILL
    }

    private val dockBorderPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = MantaTheme.BORDER_SUBTLE
        style = Paint.Style.STROKE
        strokeWidth = MantaTheme.dpF(context, 1f)
    }

    private val pillPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = MantaTheme.BG_DOCK_PILL
        style = Paint.Style.FILL
    }

    private val textPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        textSize = MantaTheme.dpF(context, 13.5f)
        typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
        textAlign = Paint.Align.LEFT
    }

    private val iconPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = MantaTheme.dpF(context, 1.9f)
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
    }

    private val iconFillPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.FILL
    }

    fun setSelectedSection(index: Int, animate: Boolean = true) {
        val clamped = index.coerceIn(0, items.size - 1)
        selectedIndex = clamped
        if (animate) {
            animatePillTo(clamped.toFloat())
        } else {
            animator?.cancel()
            pillPosition = clamped.toFloat()
            invalidate()
        }
    }

    private fun animatePillTo(target: Float) {
        animator?.cancel()
        animator = ValueAnimator.ofFloat(pillPosition, target).apply {
            duration = 220L
            interpolator = DecelerateInterpolator(1.6f)
            addUpdateListener { anim ->
                pillPosition = anim.animatedValue as Float
                invalidate()
            }
            start()
        }
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val w = width.toFloat()
        val h = height.toFloat()
        if (w <= 0f || h <= 0f) return

        val pad = MantaTheme.dpF(context, 4f)
        val outerRadius = MantaTheme.dpF(context, 20f)
        outerRect.set(pad, pad, w - pad, h - pad)

        // 1. Draw outer rounded obsidian dock container
        canvas.drawRoundRect(outerRect, outerRadius, outerRadius, dockBgPaint)
        canvas.drawRoundRect(outerRect, outerRadius, outerRadius, dockBorderPaint)

        // 2. Draw movable / slidable active highlight pill
        val innerPad = MantaTheme.dpF(context, 5f)
        val slotWidth = (outerRect.width() - innerPad * 2f) / items.size
        val pillLeft = outerRect.left + innerPad + (pillPosition * slotWidth)
        val pillTop = outerRect.top + innerPad
        val pillBottom = outerRect.bottom - innerPad
        val pillRadius = MantaTheme.dpF(context, 15f)

        pillRect.set(pillLeft, pillTop, pillLeft + slotWidth, pillBottom)
        canvas.drawRoundRect(pillRect, pillRadius, pillRadius, pillPaint)

        // 3. Draw Icons + Labels (Home | Search | Library)
        val centerY = outerRect.centerY()
        val iconSize = MantaTheme.dpF(context, 16f)
        val gap = MantaTheme.dpF(context, 8f)

        for (i in items.indices) {
            val slotCenterX = outerRect.left + innerPad + slotWidth * (i + 0.5f)
            val distance = abs(pillPosition - i.toFloat()).coerceIn(0f, 1f)
            val activeColor = blendColors(MantaTheme.TEXT_PRIMARY, MantaTheme.TEXT_MUTED, distance)

            textPaint.color = activeColor
            iconPaint.color = activeColor
            iconFillPaint.color = activeColor

            val label = items[i]
            val textWidth = textPaint.measureText(label)
            val totalContentWidth = iconSize + gap + textWidth
            val startX = slotCenterX - totalContentWidth / 2f

            drawSectionIcon(canvas, i, startX + iconSize / 2f, centerY, iconSize)

            val fontMetrics = textPaint.fontMetrics
            val textBaseline = centerY - (fontMetrics.ascent + fontMetrics.descent) / 2f
            canvas.drawText(label, startX + iconSize + gap, textBaseline, textPaint)
        }
    }

    private fun drawSectionIcon(
        canvas: Canvas,
        index: Int,
        cx: Float,
        cy: Float,
        size: Float
    ) {
        val half = size / 2f
        iconPath.reset()
        when (index) {
            0 -> {
                // Home icon (house silhouette like Muse)
                iconPath.moveTo(cx, cy - half)
                iconPath.lineTo(cx - half, cy - half * 0.1f)
                iconPath.lineTo(cx - half * 0.72f, cy - half * 0.1f)
                iconPath.lineTo(cx - half * 0.72f, cy + half * 0.85f)
                iconPath.lineTo(cx + half * 0.72f, cy + half * 0.85f)
                iconPath.lineTo(cx + half * 0.72f, cy - half * 0.1f)
                iconPath.lineTo(cx + half, cy - half * 0.1f)
                iconPath.close()
                canvas.drawPath(iconPath, iconFillPaint)
            }
            1 -> {
                // Search magnifying glass icon
                val r = half * 0.65f
                canvas.drawCircle(cx - half * 0.15f, cy - half * 0.15f, r, iconPaint)
                canvas.drawLine(
                    cx + r * 0.55f,
                    cy + r * 0.55f,
                    cx + half * 0.85f,
                    cy + half * 0.85f,
                    iconPaint
                )
            }
            2 -> {
                // Library stacked cards icon
                val r = MantaTheme.dpF(context, 3f)
                val rect = RectF(
                    cx - half * 0.65f,
                    cy - half * 0.75f,
                    cx + half * 0.75f,
                    cy + half * 0.55f
                )
                canvas.drawRoundRect(rect, r, r, iconPaint)
                canvas.drawLine(
                    cx - half * 0.9f,
                    cy - half * 0.25f,
                    cx - half * 0.9f,
                    cy + half * 0.85f,
                    iconPaint
                )
                canvas.drawLine(
                    cx - half * 0.9f,
                    cy + half * 0.85f,
                    cx + half * 0.45f,
                    cy + half * 0.85f,
                    iconPaint
                )
            }
        }
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        val w = width.toFloat()
        if (w <= 0f) return false
        val slotWidth = w / items.size

        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                animator?.cancel()
                downX = event.x
                downY = event.y
                isDraggingPill = false
                parent?.requestDisallowInterceptTouchEvent(true)
                return true
            }
            MotionEvent.ACTION_MOVE -> {
                if (abs(event.x - downX) > MantaTheme.dpF(context, 6f)) {
                    isDraggingPill = true
                }
                if (isDraggingPill) {
                    val rawPos = (event.x / slotWidth) - 0.5f
                    pillPosition = rawPos.coerceIn(0f, (items.size - 1).toFloat())
                    invalidate()
                }
                return true
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                val targetIndex = if (isDraggingPill) {
                    Math.round(pillPosition).coerceIn(0, items.size - 1)
                } else {
                    (event.x / slotWidth).toInt().coerceIn(0, items.size - 1)
                }
                val changed = targetIndex != selectedIndex
                selectedIndex = targetIndex
                animatePillTo(targetIndex.toFloat())
                if (changed || !isDraggingPill) {
                    onSectionSelected(targetIndex)
                }
                isDraggingPill = false
                return true
            }
        }
        return super.onTouchEvent(event)
    }

    private fun blendColors(active: Int, inactive: Int, ratio: Float): Int {
        val inv = 1f - ratio
        val r = ((active shr 16 and 0xFF) * inv + (inactive shr 16 and 0xFF) * ratio).toInt()
        val g = ((active shr 8 and 0xFF) * inv + (inactive shr 8 and 0xFF) * ratio).toInt()
        val b = ((active and 0xFF) * inv + (inactive and 0xFF) * ratio).toInt()
        return (0xFF shl 24) or (r shl 16) or (g shl 8) or b
    }
}
