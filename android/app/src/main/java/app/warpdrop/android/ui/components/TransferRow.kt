package app.warpdrop.android.ui.components

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.ArrowDownward
import androidx.compose.material.icons.rounded.ArrowUpward
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.FolderOpen
import androidx.compose.material.icons.rounded.Pause
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.warpdrop.android.R
import app.warpdrop.android.ui.theme.Warp
import app.warpdrop.android.util.EngineErrors
import app.warpdrop.android.util.Format
import uniffi.warp.TransferDirection
import uniffi.warp.TransferSnapshot
import uniffi.warp.TransferState

/**
 * One transfer: title → peer, a 4 dp bar, `done / total · speed · ETA`, path
 * badge, Pause/Resume (outgoing) and Cancel. Everything comes straight from
 * the engine's TransferSnapshot — no simulated progress.
 */
@Composable
fun TransferRow(
    t: TransferSnapshot,
    onPause: () -> Unit,
    onResume: () -> Unit,
    onCancel: () -> Unit,
    onOpen: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = Warp.colors
    val context = LocalContext.current
    val outgoing = t.direction == TransferDirection.OUTGOING
    val peer = t.peerName.ifBlank { stringResource(R.string.peer_unknown) }
    val terminal = t.state == TransferState.COMPLETED || t.state == TransferState.FAILED || t.state == TransferState.CANCELLED
    val openable = t.state == TransferState.COMPLETED && !outgoing && t.savedPaths.isNotEmpty()

    WarpCard(modifier.fillMaxWidth()) {
        Column(
            Modifier
                .then(if (openable) Modifier.clickable(role = Role.Button, onClick = onOpen) else Modifier)
                .padding(start = 12.dp, end = 6.dp, top = 10.dp, bottom = 12.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    if (outgoing) Icons.Rounded.ArrowUpward else Icons.Rounded.ArrowDownward,
                    contentDescription = null,
                    tint = c.textSecondary,
                    modifier = Modifier.size(16.dp),
                )
                Spacer(Modifier.width(8.dp))
                Text(
                    if (outgoing) "${t.title} → $peer" else "${t.title} ← $peer",
                    style = MaterialTheme.typography.bodyMedium,
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                if (outgoing && !terminal) {
                    when (t.state) {
                        TransferState.PAUSED -> ToolButton(Icons.Rounded.PlayArrow, stringResource(R.string.resume), onResume)
                        TransferState.TRANSFERRING, TransferState.RECONNECTING ->
                            ToolButton(Icons.Rounded.Pause, stringResource(R.string.pause), onPause)
                        else -> Unit
                    }
                }
                if (!terminal) ToolButton(Icons.Rounded.Close, stringResource(R.string.cancel), onCancel)
                if (openable) ToolButton(Icons.Rounded.FolderOpen, stringResource(R.string.show_in_folder), onOpen)
            }

            val showBar = t.totalBytes > 0uL && t.state in setOf(
                TransferState.TRANSFERRING, TransferState.PAUSED, TransferState.RECONNECTING,
                TransferState.VERIFYING, TransferState.COMPLETED,
            )
            if (showBar) {
                val fraction = (t.doneBytes.toDouble() / t.totalBytes.toDouble()).toFloat()
                ThinProgress(
                    progress = fraction,
                    color = if (t.state == TransferState.COMPLETED) c.success else c.accent,
                    modifier = Modifier.padding(end = 6.dp),
                )
            }

            Row(
                Modifier.padding(end = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                val (status, color) = statusLine(t, peer, context)
                Text(
                    status,
                    style = MaterialTheme.typography.labelSmall,
                    color = color ?: c.textSecondary,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                t.path?.let { PathBadge(it) }
            }
        }
    }
}

@Composable
private fun statusLine(t: TransferSnapshot, peer: String, context: android.content.Context): Pair<String, Color?> {
    val c = Warp.colors
    val outgoing = t.direction == TransferDirection.OUTGOING
    val sizes = "${Format.bytes(context, t.doneBytes)} / ${Format.bytes(context, t.totalBytes)}"
    return when (t.state) {
        TransferState.CONNECTING -> stringResource(R.string.state_connecting) to null
        TransferState.WAITING_FOR_ACCEPT -> stringResource(R.string.state_waiting, peer) to null
        TransferState.TRANSFERRING -> {
            val parts = mutableListOf(sizes)
            if (t.speedBps > 0uL) parts += Format.speed(context, t.speedBps)
            t.etaSecs?.let { parts += Format.eta(context, it) }
            parts.joinToString(" · ") to null
        }
        TransferState.PAUSED -> "${stringResource(R.string.state_paused)} · $sizes" to c.warning
        TransferState.RECONNECTING -> "${stringResource(R.string.state_reconnecting)} · $sizes" to c.warning
        TransferState.VERIFYING -> stringResource(R.string.state_verifying) to null
        TransferState.COMPLETED -> {
            val label = stringResource(if (outgoing) R.string.state_sent else R.string.state_saved)
            "✓ $label · ${Format.bytes(context, t.totalBytes)}" to c.success
        }
        TransferState.FAILED -> "✗ ${EngineErrors.fromCode(t.errorCode, t.error, t.peerName).resolve(context)}" to c.danger
        TransferState.CANCELLED -> stringResource(R.string.state_cancelled) to c.textTertiary
    }
}
