package app.manta.browser.ui.render

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.Typeface
import android.view.MotionEvent
import android.view.View
import app.manta.browser.core.DrawCmd
import app.manta.browser.core.ParsedMantaFrame
import app.manta.browser.ui.theme.MantaTheme
import kotlin.math.abs
import kotlin.math.max

/**
 * Hardware-accelerated Direct Canvas Renderer for Rust's `DisplayListFrame`.
 * Renders web pages parsed by `libmanta_core.so` using kilobytes of RAM,
 * supports smooth vertical scrolling, link tapping, and text copying.
 */
class MantaRenderView(
    context: Context,
    private val onLinkClicked: (String) -> Unit,
    private val onTextCopied: (String) -> Unit
) : View(context) {

    private var currentFrame: ParsedMantaFrame? = null
    private var scrollYOffset: Float = 0f
    private var lastTouchY: Float = 0f
    private var downTouchX: Float = 0f
    private var downTouchY: Float = 0f

    private val rectPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.FILL
    }
    private val textPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.FILL
    }
    private val drawRect = RectF()

    fun setFrame(frame: ParsedMantaFrame) {
        currentFrame = frame
        scrollYOffset = 0f
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val frame = currentFrame
        if (frame == null) {
            canvas.drawColor(MantaTheme.BG_OBSIDIAN)
            return
        }

        canvas.drawColor(frame.backgroundArgb)
        canvas.save()
        canvas.translate(0f, -scrollYOffset)

        for (cmd in frame.commands) {
            when (cmd) {
                is DrawCmd.RectCmd -> {
                    rectPaint.color = cmd.colorArgb
                    val left = MantaTheme.dpF(context, cmd.x)
                    val top = MantaTheme.dpF(context, cmd.y)
                    val right = left + MantaTheme.dpF(context, cmd.width)
                    val bottom = top + MantaTheme.dpF(context, cmd.height)
                    val r = MantaTheme.dpF(context, cmd.cornerRadius)
                    drawRect.set(left, top, right, bottom)
                    canvas.drawRoundRect(drawRect, r, r, rectPaint)
                }
                is DrawCmd.TextCmd -> {
                    textPaint.color = cmd.colorArgb
                    textPaint.textSize = MantaTheme.dpF(context, cmd.fontSizeSp)
                    textPaint.typeface = if (cmd.bold) {
                        Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD)
                    } else {
                        Typeface.SANS_SERIF
                    }
                    textPaint.isUnderlineText = cmd.linkHref != null

                    val xPx = MantaTheme.dpF(context, cmd.x)
                    var yPx = MantaTheme.dpF(context, cmd.y) + textPaint.textSize
                    val maxLineWidth = (width - xPx - MantaTheme.dpF(context, 16f)).coerceAtLeast(120f)

                    // Word-wrap long paragraphs cleanly
                    val words = cmd.text.split(' ')
                    val lineBuilder = StringBuilder()
                    for (word in words) {
                        val testLine = if (lineBuilder.isEmpty()) word else "$lineBuilder $word"
                        if (textPaint.measureText(testLine) > maxLineWidth && lineBuilder.isNotEmpty()) {
                            canvas.drawText(lineBuilder.toString(), xPx, yPx, textPaint)
                            lineBuilder.clear()
                            lineBuilder.append(word)
                            yPx += textPaint.textSize * 1.38f
                        } else {
                            if (lineBuilder.isNotEmpty()) lineBuilder.append(' ')
                            lineBuilder.append(word)
                        }
                    }
                    if (lineBuilder.isNotEmpty()) {
                        canvas.drawText(lineBuilder.toString(), xPx, yPx, textPaint)
                    }
                }
            }
        }

        canvas.restore()
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        val frame = currentFrame ?: return false
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                lastTouchY = event.y
                downTouchX = event.x
                downTouchY = event.y
                parent?.requestDisallowInterceptTouchEvent(true)
                return true
            }
            MotionEvent.ACTION_MOVE -> {
                val dy = lastTouchY - event.y
                lastTouchY = event.y
                val maxScroll = max(
                    0f,
                    MantaTheme.dpF(context, frame.contentHeight) - height + MantaTheme.dpF(context, 120f)
                )
                scrollYOffset = (scrollYOffset + dy).coerceIn(0f, maxScroll)
                invalidate()
                return true
            }
            MotionEvent.ACTION_UP -> {
                if (abs(event.x - downTouchX) < MantaTheme.dpF(context, 10f) &&
                    abs(event.y - downTouchY) < MantaTheme.dpF(context, 10f)
                ) {
                    handleTap(event.y + scrollYOffset, frame)
                }
                return true
            }
        }
        return super.onTouchEvent(event)
    }

    private fun handleTap(contentY: Float, frame: ParsedMantaFrame) {
        val tapDpY = contentY / resources.displayMetrics.density
        for (cmd in frame.commands) {
            if (cmd is DrawCmd.TextCmd) {
                if (tapDpY >= cmd.y - 6f && tapDpY <= cmd.y + cmd.fontSizeSp * 2.2f) {
                    if (cmd.linkHref != null) {
                        onLinkClicked(cmd.linkHref)
                        return
                    } else {
                        onTextCopied(cmd.text)
                        return
                    }
                }
            }
        }
    }
}
