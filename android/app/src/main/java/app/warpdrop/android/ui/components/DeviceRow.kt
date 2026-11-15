package app.warpdrop.android.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Computer
import androidx.compose.material.icons.rounded.ContentPaste
import androidx.compose.material.icons.rounded.DesktopWindows
import androidx.compose.material.icons.rounded.Devices
import androidx.compose.material.icons.rounded.Folder
import androidx.compose.material.icons.automirrored.rounded.InsertDriveFile
import androidx.compose.material.icons.rounded.Laptop
import androidx.compose.material.icons.rounded.LinkOff
import androidx.compose.material.icons.rounded.MoreVert
import androidx.compose.material.icons.rounded.PhoneAndroid
import androidx.compose.material.icons.rounded.PhoneIphone
import androidx.compose.material.icons.rounded.TabletMac
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.warpdrop.android.R
import app.warpdrop.android.ui.DeviceItem
import app.warpdrop.android.ui.theme.Warp
import app.warpdrop.android.util.Format
import uniffi.warp.DeviceKind
import uniffi.warp.PathKind

fun deviceIcon(kind: DeviceKind): ImageVector = when (kind) {
    DeviceKind.MAC -> Icons.Rounded.Laptop
    DeviceKind.I_PHONE -> Icons.Rounded.PhoneIphone
    DeviceKind.I_PAD -> Icons.Rounded.TabletMac
    DeviceKind.ANDROID -> Icons.Rounded.PhoneAndroid
    DeviceKind.WINDOWS -> Icons.Rounded.DesktopWindows
    DeviceKind.LINUX -> Icons.Rounded.Computer
    DeviceKind.UNKNOWN -> Icons.Rounded.Devices
}

@Composable
fun PathBadge(path: PathKind, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val cable = Format.isCable(path)
    Badge(
        text = Format.pathBadge(context, path),
        tint = if (cable) Warp.colors.cable else Warp.colors.textSecondary,
        soft = cable,
        modifier = modifier,
    )
}

/**
 * A device in My Devices / Nearby. Tap → send (staged items or the picker);
 * long-press or ⋮ → Send files / folder / clipboard, Remove.
 */
@Composable
fun DeviceRow(
    item: DeviceItem,
    onClick: () -> Unit,
    onSendFiles: () -> Unit,
    onSendFolder: () -> Unit,
    onSendClipboard: () -> Unit,
    onRemove: (() -> Unit)?,
    modifier: Modifier = Modifier,
) {
    val c = Warp.colors
    val haptics = LocalHapticFeedback.current
    var menu by remember { mutableStateOf(false) }
    WarpCard(modifier.fillMaxWidth()) {
        Box {
            Row(
                Modifier
                    .combinedClickable(
                        onClick = onClick,
                        onLongClick = {
                            haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                            menu = true
                        },
                    )
                    .padding(12.dp)
                    .alpha(if (item.online) 1f else 0.5f),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(
                    Modifier.size(44.dp).clip(RoundedCornerShape(10.dp)).background(c.surfaceHover),
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(deviceIcon(item.kind), contentDescription = null, tint = c.text, modifier = Modifier.size(24.dp))
                }
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(
                        item.name,
                        style = MaterialTheme.typography.bodyLarge,
                        fontWeight = FontWeight.SemiBold,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                        when {
                            !item.online -> Text(
                                stringResource(R.string.not_nearby),
                                style = MaterialTheme.typography.labelSmall,
                                color = c.textTertiary,
                            )
                            item.path != null -> PathBadge(item.path)
                        }
                        if (item.trusted) Badge(stringResource(R.string.badge_trusted), tint = c.success)
                    }
                }
                ToolButton(Icons.Rounded.MoreVert, stringResource(R.string.more), onClick = { menu = true })
            }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                if (item.online) {
                    MenuRow(Icons.AutoMirrored.Rounded.InsertDriveFile, stringResource(R.string.send_files)) { menu = false; onSendFiles() }
                    MenuRow(Icons.Rounded.Folder, stringResource(R.string.send_folder)) { menu = false; onSendFolder() }
                    MenuRow(Icons.Rounded.ContentPaste, stringResource(R.string.send_clipboard)) { menu = false; onSendClipboard() }
                }
                if (onRemove != null) {
                    MenuRow(Icons.Rounded.LinkOff, stringResource(R.string.remove_device), danger = true) { menu = false; onRemove() }
                }
            }
        }
    }
}

@Composable
fun MenuRow(icon: ImageVector, text: String, danger: Boolean = false, onClick: () -> Unit) {
    val tint = if (danger) Warp.colors.danger else Warp.colors.text
    DropdownMenuItem(
        text = { Text(text, color = tint, style = MaterialTheme.typography.bodyMedium) },
        leadingIcon = { Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(20.dp)) },
        onClick = onClick,
    )
}
