package app.warpdrop.android.ui.components

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.warpdrop.android.ui.theme.Warp

/** Flat card: surface fill + 1 px inner border, no elevation. */
@Composable
fun WarpCard(
    modifier: Modifier = Modifier,
    shape: Shape = Warp.cardShape,
    color: Color = Warp.colors.surface,
    borderColor: Color = Warp.colors.border,
    content: @Composable () -> Unit,
) {
    Surface(
        modifier = modifier,
        shape = shape,
        color = color,
        contentColor = Warp.colors.text,
        tonalElevation = 0.dp,
        shadowElevation = 0.dp,
        border = androidx.compose.foundation.BorderStroke(1.dp, borderColor),
        content = content,
    )
}

/** Caption-sized pill badge tinted with [tint] (text) on a soft fill. */
@Composable
fun Badge(text: String, tint: Color, modifier: Modifier = Modifier, soft: Boolean = true) {
    Box(
        modifier
            .clip(Warp.badgeShape)
            .background(if (soft) tint.copy(alpha = 0.12f) else Warp.colors.surfaceHover)
            .padding(horizontal = 6.dp, vertical = 2.dp),
    ) {
        Text(text, style = MaterialTheme.typography.labelSmall, color = tint, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

@Composable
fun SectionHeader(text: String, modifier: Modifier = Modifier, trailing: @Composable RowScope.() -> Unit = {}) {
    Row(modifier.fillMaxWidth().padding(top = 8.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(text, style = MaterialTheme.typography.titleSmall, color = Warp.colors.textSecondary, modifier = Modifier.weight(1f))
        trailing()
    }
}

/** Segmented control: tonal track, selected segment raised to `surface` with a hairline border. */
@Composable
fun SegmentedControl(options: List<String>, selected: Int, onSelect: (Int) -> Unit, modifier: Modifier = Modifier) {
    val c = Warp.colors
    Row(
        modifier
            .fillMaxWidth()
            .height(36.dp)
            .clip(Warp.controlShape)
            .background(c.surfaceHover)
            .padding(3.dp),
        horizontalArrangement = Arrangement.spacedBy(3.dp),
    ) {
        options.forEachIndexed { i, label ->
            val isSel = i == selected
            val bg by animateColorAsState(if (isSel) c.surface else Color.Transparent, label = "seg-bg")
            val border by animateColorAsState(if (isSel) c.border else Color.Transparent, label = "seg-border")
            Box(
                Modifier
                    .weight(1f)
                    .fillMaxHeight()
                    .clip(RoundedCornerShape(6.dp))
                    .background(bg)
                    .border(1.dp, border, RoundedCornerShape(6.dp))
                    .semantics {
                        role = Role.Tab
                        this.selected = isSel
                    }
                    .clickable { onSelect(i) },
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    label,
                    style = MaterialTheme.typography.labelLarge,
                    color = if (isSel) c.text else c.textSecondary,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.padding(horizontal = 6.dp),
                )
            }
        }
    }
}

/** 36 dp square icon button with an 8 dp radius (no ripple-blob look). */
@Composable
fun ToolButton(
    icon: ImageVector,
    contentDescription: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    tint: Color = Warp.colors.textSecondary,
    enabled: Boolean = true,
    size: Dp = 36.dp,
) {
    Box(
        modifier
            .size(size)
            .clip(Warp.controlShape)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .alpha(if (enabled) 1f else 0.4f),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, contentDescription = contentDescription, tint = tint, modifier = Modifier.size(20.dp))
    }
}

/** The only looping animation allowed: "searching" dot, opacity 0.4 ↔ 1 over 1.2 s. */
@Composable
fun SearchingDot(modifier: Modifier = Modifier, color: Color = Warp.colors.accent) {
    val t = rememberInfiniteTransition(label = "searching")
    val a by t.animateFloat(
        initialValue = 0.4f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(1200, easing = LinearEasing), RepeatMode.Reverse),
        label = "searching-alpha",
    )
    Box(modifier.size(8.dp).alpha(a).clip(CircleShape).background(color))
}

/** 4 dp progress bar; value changes animate over 150 ms linear. */
@Composable
fun ThinProgress(progress: Float, color: Color, modifier: Modifier = Modifier) {
    val p by animateFloatAsState(progress.coerceIn(0f, 1f), tween(150, easing = LinearEasing), label = "progress")
    Box(
        modifier
            .fillMaxWidth()
            .height(4.dp)
            .clip(RoundedCornerShape(2.dp))
            .background(Warp.colors.surfaceHover),
    ) {
        Box(Modifier.fillMaxHeight().fillMaxWidth(p).clip(RoundedCornerShape(2.dp)).background(color))
    }
}
