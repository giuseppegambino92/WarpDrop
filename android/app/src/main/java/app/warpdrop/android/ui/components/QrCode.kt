package app.warpdrop.android.ui.components

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import app.warpdrop.android.ui.theme.Warp
import uniffi.warp.QrMatrix
import uniffi.warp.qrMatrix
import kotlin.math.floor
import kotlin.math.min

/**
 * Draws `qrMatrix(payload)` as crisp squares: integer pixels per module, a
 * 4-module quiet zone, always dark-on-white (scanners expect it, even in dark mode).
 */
@Composable
fun QrCode(payload: String, contentDescription: String, modifier: Modifier = Modifier) {
    val matrix: QrMatrix? = remember(payload) { runCatching { qrMatrix(payload) }.getOrNull() }
    Box(
        modifier
            .aspectRatio(1f)
            .clip(Warp.controlShape)
            .background(Color.White)
            .semantics { this.contentDescription = contentDescription },
    ) {
        if (matrix == null) return@Box
        val n = matrix.size.toInt()
        val modules = matrix.modules
        Canvas(Modifier.fillMaxSize()) {
            val quiet = 4
            val total = n + quiet * 2
            val cell = floor(min(size.width, size.height) / total).coerceAtLeast(1f)
            val originX = floor((size.width - cell * total) / 2f) + quiet * cell
            val originY = floor((size.height - cell * total) / 2f) + quiet * cell
            for (y in 0 until n) {
                var x = 0
                while (x < n) {
                    if (!modules[y * n + x]) {
                        x++
                        continue
                    }
                    // Merge horizontal runs: fewer draw calls, no hairline seams.
                    val start = x
                    while (x < n && modules[y * n + x]) x++
                    drawRect(
                        color = Color.Black,
                        topLeft = Offset(originX + start * cell, originY + y * cell),
                        size = Size((x - start) * cell, cell),
                    )
                }
            }
        }
    }
}
