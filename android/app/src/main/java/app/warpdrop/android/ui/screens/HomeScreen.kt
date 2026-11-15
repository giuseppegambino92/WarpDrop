package app.warpdrop.android.ui.screens

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.Build
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.ContentCopy
import androidx.compose.material.icons.rounded.ContentPaste
import androidx.compose.material.icons.rounded.Folder
import androidx.compose.material.icons.rounded.Inbox
import androidx.compose.material.icons.rounded.QrCode2
import androidx.compose.material.icons.rounded.QrCodeScanner
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material.icons.rounded.UploadFile
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.warpdrop.android.R
import app.warpdrop.android.core.NodeStatus
import app.warpdrop.android.core.SendTarget
import app.warpdrop.android.core.Staged
import app.warpdrop.android.service.Notifications
import app.warpdrop.android.ui.DeviceItem
import app.warpdrop.android.ui.MainViewModel
import app.warpdrop.android.ui.PickKind
import app.warpdrop.android.ui.Screen
import app.warpdrop.android.ui.Tab
import app.warpdrop.android.ui.components.Badge
import app.warpdrop.android.ui.components.CodeField
import app.warpdrop.android.ui.components.DeviceRow
import app.warpdrop.android.ui.components.QrCode
import app.warpdrop.android.ui.components.SearchingDot
import app.warpdrop.android.ui.components.SectionHeader
import app.warpdrop.android.ui.components.SegmentedControl
import app.warpdrop.android.ui.components.ToolButton
import app.warpdrop.android.ui.components.TransferRow
import app.warpdrop.android.ui.components.WarpCard
import app.warpdrop.android.ui.components.rememberNow
import app.warpdrop.android.ui.theme.Warp
import app.warpdrop.android.util.Format
import kotlinx.coroutines.launch
import uniffi.warp.CodeInfo
import uniffi.warp.PathKind
import uniffi.warp.TransferSnapshot
import uniffi.warp.TransferState

@Composable
fun HomeScreen(
    vm: MainViewModel,
    snackbar: SnackbarHostState,
    banners: LazyListScope.() -> Unit,
) {
    val c = Warp.colors
    val context = LocalContext.current
    val resources = LocalResources.current
    val scope = rememberCoroutineScope()

    val tab by vm.tab.collectAsStateWithLifecycle()
    val code by vm.code.collectAsStateWithLifecycle()
    val digits by vm.codeInput.collectAsStateWithLifecycle()
    val myDevices by vm.myDevices.collectAsStateWithLifecycle()
    val nearby by vm.nearby.collectAsStateWithLifecycle()
    val transfers by vm.transfers.collectAsStateWithLifecycle()
    val staged by vm.staged.collectAsStateWithLifecycle()
    val preparing by vm.preparing.collectAsStateWithLifecycle()
    val status by vm.status.collectAsStateWithLifecycle()
    val interfaces by vm.interfaces.collectAsStateWithLifecycle()

    val cableUp = interfaces.any { it.kind == PathKind.USB_CABLE || it.kind == PathKind.THUNDERBOLT_BRIDGE } ||
        (myDevices + nearby).any { Format.isCable(it.path) }

    val copyCode: () -> Unit = {
        code?.let { info ->
            context.getSystemService(ClipboardManager::class.java)
                ?.setPrimaryClip(ClipData.newPlainText(resources.getString(R.string.your_code), info.code))
            // Android 13+ confirms clipboard writes itself.
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
                scope.launch { snackbar.showSnackbar(resources.getString(R.string.code_copied)) }
            }
        }
    }

    Scaffold(
        containerColor = c.bg,
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            Row(
                Modifier
                    .fillMaxWidth()
                    .background(c.bg)
                    .windowInsetsPadding(WindowInsets.statusBars)
                    .padding(start = 16.dp, end = 8.dp, top = 8.dp, bottom = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(stringResource(R.string.app_name), style = MaterialTheme.typography.titleLarge, modifier = Modifier.weight(1f))
                if (cableUp) {
                    Badge(stringResource(R.string.badge_usb), tint = c.cable)
                    Spacer(Modifier.width(4.dp))
                }
                ToolButton(Icons.Rounded.Add, stringResource(R.string.link_device), onClick = vm::openLink)
                ToolButton(Icons.Rounded.Settings, stringResource(R.string.settings), onClick = { vm.show(Screen.SETTINGS) })
            }
        },
    ) { padding ->
        LazyColumn(
            Modifier.fillMaxSize().imePadding(),
            contentPadding = PaddingValues(
                start = Warp.pagePadding,
                end = Warp.pagePadding,
                top = padding.calculateTopPadding(),
                bottom = padding.calculateBottomPadding() + 24.dp,
            ),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            item(key = "segments") {
                SegmentedControl(
                    options = listOf(
                        stringResource(R.string.tab_my_devices),
                        stringResource(R.string.tab_nearby),
                        stringResource(R.string.tab_quick_code),
                    ),
                    selected = tab.ordinal,
                    onSelect = { vm.selectTab(Tab.entries[it]) },
                    modifier = Modifier.padding(bottom = 4.dp),
                )
            }

            banners()

            if (status is NodeStatus.Failed) {
                item(key = "engine-failed") {
                    Notice(stringResource(R.string.error_engine_start, (status as NodeStatus.Failed).message), tint = c.danger)
                }
            }

            staged?.let { s ->
                item(key = "staged") {
                    StagedBanner(s, onClear = vm::clearStaged)
                }
            }
            if (preparing) {
                item(key = "preparing") {
                    Row(Modifier.padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                        CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp, color = c.accent)
                        Spacer(Modifier.width(8.dp))
                        Text(stringResource(R.string.preparing), style = MaterialTheme.typography.labelSmall, color = c.textSecondary)
                    }
                }
            }

            if (tab != Tab.QUICK_CODE) {
                item(key = "codebar") {
                    CodeBar(
                        code = code,
                        digits = digits,
                        onDigits = vm::onCodeInput,
                        onSubmit = vm::submitCode,
                        onCopy = copyCode,
                        onQr = { vm.showQr(true) },
                        onScan = { vm.show(Screen.SCANNER) },
                    )
                }
            }

            when (tab) {
                Tab.MY_DEVICES -> {
                    item(key = "my-header") { SectionHeader(stringResource(R.string.tab_my_devices)) }
                    if (myDevices.isEmpty()) {
                        item(key = "my-empty") {
                            EmptyState(
                                text = stringResource(R.string.no_my_devices),
                                action = stringResource(R.string.link_device),
                                onAction = vm::openLink,
                            )
                        }
                    } else {
                        deviceItems(myDevices, vm, removable = true)
                    }
                }
                Tab.NEARBY -> {
                    item(key = "nearby-header") {
                        SectionHeader(stringResource(R.string.tab_nearby)) {
                            SearchingDot(Modifier.padding(end = 8.dp))
                        }
                    }
                    if (nearby.isEmpty()) {
                        item(key = "nearby-empty") { Searching() }
                    } else {
                        deviceItems(nearby, vm, removable = false)
                    }
                }
                Tab.QUICK_CODE -> {
                    item(key = "qc-mine") { MyCodePanel(code, onCopy = copyCode) }
                    item(key = "qc-send") { SendToCodePanel(vm, digits) }
                }
            }

            if (transfers.isNotEmpty()) {
                item(key = "transfers-header") {
                    SectionHeader(stringResource(R.string.notif_channel_transfers), Modifier.padding(top = 8.dp)) {
                        if (transfers.any { it.state == TransferState.COMPLETED || it.state == TransferState.FAILED || it.state == TransferState.CANCELLED }) {
                            TextButton(onClick = vm::clearFinished, shape = Warp.controlShape) {
                                Text(stringResource(R.string.clear_finished), color = c.accent)
                            }
                        }
                    }
                }
                items(transfers, key = { "t:" + it.id }) { t ->
                    TransferRow(
                        t = t,
                        onPause = { vm.pause(t.id) },
                        onResume = { vm.resume(t.id) },
                        onCancel = { vm.cancel(t.id) },
                        onOpen = { openTransfer(context, t) },
                        modifier = Modifier.animateItem(placementSpec = Warp.spring()),
                    )
                }
            }
        }
    }
}

private fun LazyListScope.deviceItems(list: List<DeviceItem>, vm: MainViewModel, removable: Boolean) {
    items(list, key = { "d:" + it.id }) { d ->
        val target = SendTarget.Peer(d.id, d.name)
        DeviceRow(
            item = d,
            onClick = { vm.onDeviceClick(d) },
            onSendFiles = { vm.requestPick(target, PickKind.FILES) },
            onSendFolder = { vm.requestPick(target, PickKind.FOLDER) },
            onSendClipboard = { vm.sendClipboard(target) },
            onRemove = if (removable) ({ vm.untrust(d) }) else null,
            modifier = Modifier.animateItem(placementSpec = Warp.spring()),
        )
    }
}

private fun openTransfer(context: Context, t: TransferSnapshot) {
    try {
        context.startActivity(Intent.createChooser(Notifications.viewIntent(context, t.savedPaths), null))
    } catch (e: ActivityNotFoundException) {
        // Nothing can open it; the file is still in Download/WarpDrop.
    }
}

@Composable
private fun CodeBar(
    code: CodeInfo?,
    digits: String,
    onDigits: (String) -> Unit,
    onSubmit: () -> Unit,
    onCopy: () -> Unit,
    onQr: () -> Unit,
    onScan: () -> Unit,
) {
    val c = Warp.colors
    val now = rememberNow()
    WarpCard(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(stringResource(R.string.your_code), style = MaterialTheme.typography.labelSmall, color = c.textSecondary)
                    Text(code?.display ?: "———", style = Warp.codeStyle.copy(fontSize = 24.sp), color = c.text)
                    if (code != null) {
                        Text(
                            stringResource(R.string.code_expires_in, Format.countdown(code.expiresAtMs.toLong() - now)),
                            style = MaterialTheme.typography.labelSmall,
                            color = c.textTertiary,
                        )
                    }
                }
                ToolButton(Icons.Rounded.ContentCopy, stringResource(R.string.copy_code), onCopy, enabled = code != null)
                ToolButton(Icons.Rounded.QrCode2, stringResource(R.string.show_qr), onQr, enabled = code != null)
            }
            Text(stringResource(R.string.send_to_code), style = MaterialTheme.typography.labelSmall, color = c.textSecondary)
            Row(verticalAlignment = Alignment.CenterVertically) {
                CodeField(
                    digits = digits,
                    onChange = onDigits,
                    onSubmit = onSubmit,
                    label = stringResource(R.string.send_to_code),
                    submitDescription = stringResource(R.string.send),
                    modifier = Modifier.weight(1f),
                )
                Spacer(Modifier.width(4.dp))
                ToolButton(Icons.Rounded.QrCodeScanner, stringResource(R.string.scan_qr), onScan, size = 48.dp)
            }
        }
    }
}

@Composable
private fun MyCodePanel(code: CodeInfo?, onCopy: () -> Unit) {
    val c = Warp.colors
    val now = rememberNow()
    WarpCard(Modifier.fillMaxWidth()) {
        Column(Modifier.fillMaxWidth().padding(16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Text(stringResource(R.string.your_code), style = MaterialTheme.typography.titleSmall, color = c.textSecondary)
            Spacer(Modifier.height(4.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(code?.display ?: "———", style = Warp.codeStyle.copy(fontSize = 36.sp), color = c.text)
                Spacer(Modifier.width(4.dp))
                ToolButton(Icons.Rounded.ContentCopy, stringResource(R.string.copy_code), onCopy, enabled = code != null)
            }
            if (code != null) {
                Spacer(Modifier.height(12.dp))
                QrCode(code.qrPayload, stringResource(R.string.show_qr), Modifier.width(220.dp))
                Spacer(Modifier.height(10.dp))
                Text(
                    stringResource(R.string.code_expires_in, Format.countdown(code.expiresAtMs.toLong() - now)),
                    style = MaterialTheme.typography.labelSmall,
                    color = c.textTertiary,
                )
            }
        }
    }
}

@Composable
private fun SendToCodePanel(vm: MainViewModel, digits: String) {
    val c = Warp.colors
    val ready = digits.length == 6
    WarpCard(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(stringResource(R.string.send_to_code), style = MaterialTheme.typography.titleSmall, color = c.text)
            Text(stringResource(R.string.send_to_code_hint), style = MaterialTheme.typography.bodyMedium, color = c.textSecondary)
            CodeField(
                digits = digits,
                onChange = vm::onCodeInput,
                onSubmit = vm::submitCode,
                label = stringResource(R.string.send_to_code),
                submitDescription = stringResource(R.string.send),
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                PanelButton(Icons.Rounded.UploadFile, stringResource(R.string.send_files), ready, Modifier.weight(1f)) {
                    vm.codeTarget()?.let { vm.requestPick(it, PickKind.FILES) }
                }
                PanelButton(Icons.Rounded.Folder, stringResource(R.string.send_folder), ready, Modifier.weight(1f)) {
                    vm.codeTarget()?.let { vm.requestPick(it, PickKind.FOLDER) }
                }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                PanelButton(Icons.Rounded.ContentPaste, stringResource(R.string.send_clipboard), ready, Modifier.weight(1f)) {
                    vm.codeTarget()?.let(vm::sendClipboard)
                }
                PanelButton(Icons.Rounded.QrCodeScanner, stringResource(R.string.scan_qr), true, Modifier.weight(1f)) {
                    vm.show(Screen.SCANNER)
                }
            }
        }
    }
}

@Composable
private fun PanelButton(icon: ImageVector, text: String, enabled: Boolean, modifier: Modifier, onClick: () -> Unit) {
    val c = Warp.colors
    OutlinedButton(
        onClick = onClick,
        enabled = enabled,
        shape = Warp.controlShape,
        border = androidx.compose.foundation.BorderStroke(1.dp, c.border),
        contentPadding = PaddingValues(horizontal = 10.dp, vertical = 8.dp),
        colors = ButtonDefaults.outlinedButtonColors(contentColor = c.text, disabledContentColor = c.textTertiary),
        modifier = modifier,
    ) {
        Icon(icon, contentDescription = null, modifier = Modifier.size(18.dp))
        Spacer(Modifier.width(6.dp))
        Text(text, style = MaterialTheme.typography.labelLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

@Composable
private fun StagedBanner(staged: Staged, onClear: () -> Unit) {
    val c = Warp.colors
    val context = LocalContext.current
    WarpCard(Modifier.fillMaxWidth(), color = c.accentSoft, borderColor = c.accent.copy(alpha = 0.4f)) {
        Row(Modifier.padding(start = 12.dp, end = 4.dp, top = 4.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(
                if (staged is Staged.Clip) Icons.Rounded.ContentPaste else Icons.Rounded.Inbox,
                contentDescription = null,
                tint = c.accent,
                modifier = Modifier.size(20.dp),
            )
            Spacer(Modifier.width(10.dp))
            Text(
                stringResource(R.string.staged_items, staged.label.resolve(context)),
                style = MaterialTheme.typography.bodyMedium,
                color = c.text,
                modifier = Modifier.weight(1f).padding(vertical = 8.dp),
            )
            ToolButton(Icons.Rounded.Close, stringResource(R.string.cancel), onClear)
        }
    }
}

@Composable
fun Notice(text: String, tint: androidx.compose.ui.graphics.Color, action: String? = null, onAction: () -> Unit = {}) {
    WarpCard(Modifier.fillMaxWidth(), color = tint.copy(alpha = 0.08f), borderColor = tint.copy(alpha = 0.3f)) {
        Row(Modifier.padding(start = 12.dp, end = 8.dp, top = 6.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(
                text,
                style = MaterialTheme.typography.bodyMedium,
                color = Warp.colors.text,
                modifier = Modifier.weight(1f).padding(vertical = 6.dp),
            )
            if (action != null) {
                TextButton(onClick = onAction, shape = Warp.controlShape) { Text(action, color = Warp.colors.accent) }
            }
        }
    }
}

@Composable
private fun EmptyState(text: String, action: String? = null, onAction: () -> Unit = {}) {
    val c = Warp.colors
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text(text, style = MaterialTheme.typography.bodyMedium, color = c.textSecondary, textAlign = TextAlign.Center)
        if (action != null) {
            Button(onClick = onAction, shape = Warp.controlShape, colors = ButtonDefaults.buttonColors(containerColor = c.accent)) {
                Icon(Icons.Rounded.Add, contentDescription = null, modifier = Modifier.size(18.dp))
                Spacer(Modifier.width(6.dp))
                Text(action)
            }
        }
    }
}

@Composable
private fun Searching() {
    val c = Warp.colors
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            SearchingDot()
            Spacer(Modifier.width(8.dp))
            Text(stringResource(R.string.searching), style = MaterialTheme.typography.bodyMedium, color = c.text)
        }
        Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
            Text(stringResource(R.string.no_nearby), style = MaterialTheme.typography.bodyMedium, color = c.textSecondary, textAlign = TextAlign.Center)
        }
    }
}
