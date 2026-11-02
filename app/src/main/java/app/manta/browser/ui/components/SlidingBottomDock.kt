package app.manta.browser.ui.components

import android.animation.ValueAnimator
import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.Typeface
import android.view.MotionEvent
import android.view.View
import android.view.animation.DecelerateInterpolator
import app.manta.browser.ui.theme.MantaTheme
import kotlin.math.abs

/**
 * M3 Expressive Unified Gesture Browser Dock:
 * - Ultra-minimalist borderless floating pill bar at the bottom of the screen.
 * - Gestures:
 *   1. Swipe Left / Right across the dock -> Switches to Previous / Next open tab!
 *   2. Swipe Up on the dock -> Opens the M3 Expressive Tab Chooser Grid!
 *   3. Tap Center Address Pill -> Opens the floating Google Omnibox overlay.
 *   4. Tap Tab Count Squircle (`[ N ]`) -> Opens the M3 Expressive Tab Chooser Grid.
 *   5. Tap Quick Menu (`•••`) -> Opens the borderless Engine & Shield Control Sheet.
 */
class SlidingBottomDock(
    context: Context,
    private val onAddressBarTap: () -> Unit,
    private val onTabSwitcherTap: () -> Unit,
    private val onMenuTap: () -> Unit,
    private val onHorizontalTabSwipe: (direction: Int) -> Unit,
    private val onBackTap: () -> Unit
) : View(context) {

    private var displayTitle: String = "Search Google or type URL"
    private var tabCount: Int = 1
    private var loadProgress: Int = 100
    private var corsActive: Boolean = true

    // Gesture tracking state
    private var downX: Float = 0f
    private var downY: Float = 0f
    private var dragOffsetX: Float = 0f
    private var dragOffsetY: Float = 0f
    private var isGestureHandled: Boolean = false
    private var snapAnimator: ValueAnimator? = null

    private val dockRect = RectF()
    private val omniboxRect = RectF()
    private val tabBadgeRect = RectF()
    private val backCircleRect = RectF()
    private val menuCircleRect = RectF()
    private val progressRect = RectF()

    private val dockPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = MantaTheme.BG_DOCK
        style = Paint.Style.FILL
    }

    private val omniboxPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = MantaTheme.BG_DOCK_PILL
        style = Paint.Style.FILL
    }

    private val accentCirclePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = MantaTheme.CONTAINER_PRIMARY
        style = Paint.Style.FILL
    }

    private val subtleButtonPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = MantaTheme.BG_CONTAINER_HIGH
        style = Paint.Style.FILL
    }

    private val progressPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = MantaTheme.ACCENT_PRIMARY
        style = Paint.Style.FILL
    }

    private val titlePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = MantaTheme.TEXT_PRIMARY
        textSize = MantaTheme.dpF(context, 13.5f)
        typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
        textAlign = Paint.Align.CENTER
    }

    private val badgeTextPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = MantaTheme.ON_CONTAINER_PRIMARY
        textSize = MantaTheme.dpF(context, 12f)
        typeface = Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
        textAlign = Paint.Align.CENTER
    }

    private val iconPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = MantaTheme.TEXT_PRIMARY
        style = Paint.Style.STROKE
        strokeWidth = MantaTheme.dpF(context, 2f)
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
    }

    private val dotPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = MantaTheme.TEXT_PRIMARY
        style = Paint.Style.FILL
    }

    fun updateDockState(
        urlOrTitle: String,
        tabsCount: Int,
        progress: Int,
        isCorsEnabled: Boolean
    ) {
        displayTitle = MantaTheme.extractCleanHost(urlOrTitle)
        tabCount = tabsCount.coerceAtLeast(1)
        loadProgress = progress
        corsActive = isCorsEnabled
        invalidate()
    }

    /**
     * Smoothly hides or reveals the floating dock when the user scrolls the webpage
     * so the website gets 100% full-screen space.
     */
    fun setDockVisible(visible: Boolean) {
        val targetY = if (visible) 0f else MantaTheme.dpF(context, 92f)
        animate()
            .translationY(targetY)
            .setDuration(210L)
            .setInterpolator(DecelerateInterpolator(1.5f))
            .start()
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val w = width.toFloat()
        val h = height.toFloat()
        if (w <= 0f || h <= 0f) return

        val padH = MantaTheme.dpF(context, 4f)
        val padV = MantaTheme.dpF(context, 3f)
        val dockRadius = h / 2f

        canvas.save()
        canvas.translate(dragOffsetX * 0.25f, dragOffsetY.coerceAtMost(0f) * 0.35f)

        // 1. Borderless M3 Expressive Outer Pill Surface
        dockRect.set(padH, padV, w - padH, h - padV)
        canvas.drawRoundRect(dockRect, dockRadius, dockRadius, dockPaint)

        val innerMargin = MantaTheme.dpF(context, 6f)
        val btnSize = dockRect.height() - innerMargin * 2f
        val cy = dockRect.centerY()

        // 2. Left Back / Home Circular Pill
        val backLeft = dockRect.left + innerMargin
        backCircleRect.set(backLeft, cy - btnSize / 2f, backLeft + btnSize, cy + btnSize / 2f)
        canvas.drawOval(backCircleRect, subtleButtonPaint)
        drawBackChevron(canvas, backCircleRect.centerX(), cy, MantaTheme.dpF(context, 5.5f))

        // 3. Right Menu Circular Pill (`•••`)
        val menuRight = dockRect.right - innerMargin
        menuCircleRect.set(menuRight - btnSize, cy - btnSize / 2f, menuRight, cy + btnSize / 2f)
        canvas.drawOval(menuCircleRect, subtleButtonPaint)
        drawMenuDots(canvas, menuCircleRect.centerX(), cy)

        // 4. Tab Counter Squircle (`[ N ]`)
        val gap = MantaTheme.dpF(context, 6f)
        val tabRight = menuCircleRect.left - gap
        tabBadgeRect.set(tabRight - btnSize, cy - btnSize / 2f, tabRight, cy + btnSize / 2f)
        val squircleR = MantaTheme.dpF(context, 13f)
        canvas.drawRoundRect(tabBadgeRect, squircleR, squircleR, accentCirclePaint)

        val fmBadge = badgeTextPaint.fontMetrics
        val badgeBaseline = cy - (fmBadge.ascent + fmBadge.descent) / 2f
        canvas.drawText(tabCount.toString(), tabBadgeRect.centerX(), badgeBaseline, badgeTextPaint)

        // 5. Center Expressive Omnibox Pill (Draggable horizontally to switch tabs!)
        val omniLeft = backCircleRect.right + gap
        val omniRight = tabBadgeRect.left - gap
        omniboxRect.set(
            omniLeft + dragOffsetX * 0.45f,
            cy - btnSize / 2f,
            omniRight + dragOffsetX * 0.45f,
            cy + btnSize / 2f
        )
        canvas.drawRoundRect(omniboxRect, btnSize / 2f, btnSize / 2f, omniboxPaint)

        // Subtle loading progress bar inside the Omnibox pill
        if (loadProgress in 1..99) {
            val progW = omniboxRect.width() * (loadProgress / 100f)
            progressRect.set(
                omniboxRect.left + MantaTheme.dpF(context, 12f),
                omniboxRect.bottom - MantaTheme.dpF(context, 3f),
                (omniboxRect.left + progW).coerceAtMost(omniboxRect.right - MantaTheme.dpF(context, 12f)),
                omniboxRect.bottom - MantaTheme.dpF(context, 1f)
            )
            canvas.drawRoundRect(progressRect, 4f, 4f, progressPaint)
        }

        // Address / Search Label
        val isPlaceholder = displayTitle.startsWith("Search Google")
        titlePaint.color = if (isPlaceholder) MantaTheme.TEXT_SECONDARY else MantaTheme.TEXT_PRIMARY
        val maxChars = ((omniboxRect.width() / MantaTheme.dpF(context, 7.8f)).toInt()).coerceAtLeast(10)
        val clippedTitle = if (displayTitle.length > maxChars) {
            displayTitle.take(maxChars - 1) + "…"
        } else {
            displayTitle
        }
        val fmTitle = titlePaint.fontMetrics
        val titleBaseline = cy - (fmTitle.ascent + fmTitle.descent) / 2f
        canvas.drawText(clippedTitle, omniboxRect.centerX(), titleBaseline, titlePaint)

        canvas.restore()
    }

    private fun drawBackChevron(canvas: Canvas, cx: Float, cy: Float, size: Float) {
        canvas.drawLine(cx + size * 0.4f, cy - size, cx - size * 0.6f, cy, iconPaint)
        canvas.drawLine(cx - size * 0.6f, cy, cx + size * 0.4f, cy + size, iconPaint)
    }

    private fun drawMenuDots(canvas: Canvas, cx: Float, cy: Float) {
        val r = MantaTheme.dpF(context, 1.8f)
        val step = MantaTheme.dpF(context, 5f)
        dotPaint.color = if (corsActive) MantaTheme.ACCENT_EMERALD else MantaTheme.TEXT_PRIMARY
        canvas.drawCircle(cx - step, cy, r, dotPaint)
        dotPaint.color = MantaTheme.TEXT_PRIMARY
        canvas.drawCircle(cx, cy, r, dotPaint)
        canvas.drawCircle(cx + step, cy, r, dotPaint)
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                snapAnimator?.cancel()
                downX = event.x
                downY = event.y
                dragOffsetX = 0f
                dragOffsetY = 0f
                isGestureHandled = false
                parent?.requestDisallowInterceptTouchEvent(true)
                return true
            }
            MotionEvent.ACTION_MOVE -> {
                dragOffsetX = event.x - downX
                dragOffsetY = event.y - downY

                // Swipe Up Gesture -> Open Tab Switcher Grid immediately!
                if (!isGestureHandled && dragOffsetY < -MantaTheme.dpF(context, 38f) && abs(dragOffsetY) > abs(dragOffsetX)) {
                    isGestureHandled = true
                    animateBackToCenter()
                    onTabSwitcherTap()
                    return true
                }
                invalidate()
                return true
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                if (!isGestureHandled) {
                    val swipeThreshold = MantaTheme.dpF(context, 48f)
                    val tapSlop = MantaTheme.dpF(context, 12f)

                    if (abs(dragOffsetX) > swipeThreshold && abs(dragOffsetX) > abs(dragOffsetY)) {
                        // Horizontal Swipe across Dock -> Switch to Previous / Next Tab
                        val dir = if (dragOffsetX < 0) 1 else -1
                        onHorizontalTabSwipe(dir)
                    } else if (abs(dragOffsetX) < tapSlop && abs(dragOffsetY) < tapSlop) {
                        handleTap(event.x, event.y)
                    }
                }
                animateBackToCenter()
                return true
            }
        }
        return super.onTouchEvent(event)
    }

    private fun handleTap(x: Float, y: Float) {
        when {
            backCircleRect.contains(x, y) -> onBackTap()
            tabBadgeRect.contains(x, y) -> onTabSwitcherTap()
            menuCircleRect.contains(x, y) -> onMenuTap()
            else -> onAddressBarTap()
        }
    }

    private fun animateBackToCenter() {
        val startX = dragOffsetX
        val startY = dragOffsetY
        snapAnimator?.cancel()
        snapAnimator = ValueAnimator.ofFloat(1f, 0f).apply {
            duration = 200L
            interpolator = DecelerateInterpolator(1.6f)
            addUpdateListener { anim ->
                val f = anim.animatedValue as Float
                dragOffsetX = startX * f
                dragOffsetY = startY * f
                invalidate()
            }
            start()
        }
    }
}
