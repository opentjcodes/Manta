package app.manta.browser

import android.app.Activity
import android.os.Bundle
import android.widget.TextView
import app.manta.browser.core.MantaNative

/**
 * Minimal bootstrap host to verify `libmanta_core.so` JNI linkage in Commit #1.
 * Next step: full Kotlin UI with the Muse-style sliding bottom dock (Home | Search | Library).
 */
class MainActivity : Activity() {
    private var activeTabId: Int = 0

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        activeTabId = MantaNative.initEngine(true)

        val frameOutput = MantaNative.streamHtmlChunk(
            activeTabId,
            "https://manta.local/boot",
            "<html><body><h1>Manta Rust Core Active</h1><p>Zero-Cache Streaming Engine Ready</p></body></html>",
            true
        )

        val view = TextView(this).apply {
            setBackgroundColor(0xFF09090B.toInt())
            setTextColor(0xFFF8FAFC.toInt())
            textSize = 13f
            setPadding(48, 96, 48, 48)
            text = frameOutput
        }
        setContentView(view)
    }

    override fun onStop() {
        super.onStop()
        if (isFinishing) {
            MantaNative.purgeAllVolatileRam()
        }
    }

    override fun onDestroy() {
        MantaNative.purgeAllVolatileRam()
        super.onDestroy()
    }
}
