package app.warpdrop.android.ui.screens

import android.app.DownloadManager
import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.rounded.ContentCopy
import androidx.compose.material.icons.rounded.FolderOpen
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.RadioButton
import androidx.compose.material3.RadioButtonDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.warpdrop.android.BuildConfig
import app.warpdrop.android.R
import app.warpdrop.android.ui.MainViewModel
import app.warpdrop.android.ui.Screen
import app.warpdrop.android.ui.components.SectionHeader
import app.warpdrop.android.ui.components.ToolButton
import app.warpdrop.android.ui.components.WarpCard
import app.warpdrop.android.ui.theme.Warp
import app.warpdrop.android.util.Locales
import uniffi.warp.coreVersion

@Composable
fun SettingsScreen(vm: MainViewModel) {
    val c = Warp.colors
    val context = LocalContext.current
    val focus = LocalFocusManager.current
    val resources = LocalResources.current
    val identity by vm.identity.collectAsStateWithLifecycle()
    val downloadDir by vm.downloadDir.collectAsStateWithLifecycle()
    BackHandler { vm.show(Screen.HOME) }

    Column(
        Modifier
            .fillMaxSize()
            .background(c.bg)
            .windowInsetsPadding(WindowInsets.statusBars)
            .imePadding(),
    ) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            ToolButton(Icons.AutoMirrored.Rounded.ArrowBack, stringResource(R.string.back), { vm.show(Screen.HOME) }, tint = c.text)
            Spacer(Modifier.width(4.dp))
            Text(stringResource(R.string.settings), style = MaterialTheme.typography.titleLarge)
        }
        Column(
            Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = Warp.pagePadding)
                .navigationBarsPadding()
                .padding(bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            // Device name
            SectionHeader(stringResource(R.string.settings_device_name))
            var name by rememberSaveable(identity?.name) { mutableStateOf(identity?.name.orEmpty()) }
            val commitName = { if (name.isNotBlank() && name != identity?.name) vm.setDeviceName(name) }
            OutlinedTextField(
                value = name,
                onValueChange = { name = it.take(60) },
                singleLine = true,
                shape = Warp.controlShape,
                colors = fieldColors(),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { commitName(); focus.clearFocus() }),
                modifier = Modifier.fillMaxWidth().onFocusChanged { if (!it.isFocused) commitName() },
            )

            // Language
            SectionHeader(stringResource(R.string.settings_language), Modifier.padding(top = 12.dp))
            var lang by remember { mutableStateOf(vm.language()) }
            WarpCard(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(vertical = 4.dp)) {
                    listOf(
                        Locales.Choice.SYSTEM to stringResource(R.string.settings_language_system),
                        Locales.Choice.ENGLISH to stringResource(R.string.language_english),
                        Locales.Choice.ITALIAN to stringResource(R.string.language_italian),
                    ).forEach { (choice, label) ->
                        Row(
                            Modifier
                                .fillMaxWidth()
                                .clickable(role = Role.RadioButton) {
                                    lang = choice
                                    vm.setLanguage(choice)
                                }
                                .padding(horizontal = 4.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            RadioButton(
                                selected = lang == choice,
                                onClick = null,
                                colors = RadioButtonDefaults.colors(selectedColor = c.accent, unselectedColor = c.textTertiary),
                                modifier = Modifier.padding(12.dp),
                            )
                            Text(label, style = MaterialTheme.typography.bodyLarge)
                        }
                    }
                }
            }

            // Relay
            SectionHeader(stringResource(R.string.settings_relay), Modifier.padding(top = 12.dp))
            var relay by rememberSaveable { mutableStateOf(vm.relayUrl()) }
            var savedRelay by rememberSaveable { mutableStateOf(relay) }
            val commitRelay = {
                if (relay.trim() != savedRelay.trim()) {
                    savedRelay = relay
                    vm.setRelayUrl(relay)
                }
            }
            OutlinedTextField(
                value = relay,
                onValueChange = { relay = it },
                singleLine = true,
                placeholder = { Text("wss://relay.example.com", color = c.textTertiary) },
                shape = Warp.controlShape,
                colors = fieldColors(),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { commitRelay(); focus.clearFocus() }),
                modifier = Modifier.fillMaxWidth().onFocusChanged { if (!it.isFocused) commitRelay() },
            )
            Text(
                stringResource(R.string.settings_relay_hint),
                style = MaterialTheme.typography.labelSmall,
                color = c.textSecondary,
                modifier = Modifier.padding(top = 4.dp, start = 4.dp),
            )

            // Fingerprint
            SectionHeader(stringResource(R.string.settings_fingerprint), Modifier.padding(top = 12.dp))
            WarpCard(Modifier.fillMaxWidth()) {
                Row(Modifier.padding(start = 12.dp, end = 4.dp, top = 4.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        identity?.fingerprint ?: "—",
                        style = MaterialTheme.typography.bodyLarge.copy(fontFamily = FontFamily.Monospace),
                        modifier = Modifier.weight(1f).padding(vertical = 10.dp),
                    )
                    identity?.let { id ->
                        ToolButton(Icons.Rounded.ContentCopy, stringResource(R.string.copy_code), {
                            context.getSystemService(ClipboardManager::class.java)
                                ?.setPrimaryClip(ClipData.newPlainText(resources.getString(R.string.settings_fingerprint), id.fingerprint))
                        })
                    }
                }
            }

            // Download folder
            SectionHeader(stringResource(R.string.settings_download_folder), Modifier.padding(top = 12.dp))
            WarpCard(Modifier.fillMaxWidth()) {
                Row(Modifier.padding(start = 12.dp, end = 4.dp, top = 4.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        downloadDir.ifEmpty { "—" }.replace("/storage/emulated/0/", ""),
                        style = MaterialTheme.typography.bodyMedium,
                        color = c.text,
                        modifier = Modifier.weight(1f).padding(vertical = 10.dp),
                    )
                    ToolButton(Icons.Rounded.FolderOpen, stringResource(R.string.show_in_folder), {
                        try {
                            context.startActivity(Intent(DownloadManager.ACTION_VIEW_DOWNLOADS))
                        } catch (e: ActivityNotFoundException) {
                            // No Files app.
                        }
                    })
                }
            }

            Text(
                "${stringResource(R.string.settings_version)} ${BuildConfig.VERSION_NAME} · core ${remember { runCatching { coreVersion() }.getOrDefault("?") }}",
                style = MaterialTheme.typography.labelSmall,
                color = c.textTertiary,
                modifier = Modifier.padding(top = 20.dp, start = 4.dp),
            )
        }
    }
}

@Composable
private fun fieldColors() = OutlinedTextFieldDefaults.colors(
    focusedBorderColor = Warp.colors.accent,
    unfocusedBorderColor = Warp.colors.border,
    focusedContainerColor = Warp.colors.surface,
    unfocusedContainerColor = Warp.colors.surface,
    cursorColor = Warp.colors.accent,
)
