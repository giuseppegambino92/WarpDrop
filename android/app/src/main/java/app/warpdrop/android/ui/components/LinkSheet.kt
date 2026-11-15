package app.warpdrop.android.ui.components

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import app.warpdrop.android.R
import app.warpdrop.android.ui.LinkMode
import app.warpdrop.android.ui.LinkSheetState
import app.warpdrop.android.ui.theme.Warp
import app.warpdrop.android.util.Format
import uniffi.warp.CodeInfo

/** "Link My Device": Show my code (`openLinkWindow(300)`) / Enter code (`linkWithCode`). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LinkSheet(
    state: LinkSheetState,
    code: CodeInfo?,
    onMode: (LinkMode) -> Unit,
    onInput: (String) -> Unit,
    onSubmit: () -> Unit,
    onDismiss: () -> Unit,
) {
    val c = Warp.colors
    val context = LocalContext.current
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = c.surface,
        contentColor = c.text,
        tonalElevation = 0.dp,
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .padding(horizontal = 20.dp)
                .padding(bottom = 24.dp)
                .navigationBarsPadding(),
            verticalArrangement = Arrangement.spacedBy(16.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(stringResource(R.string.link_device), style = MaterialTheme.typography.titleLarge, modifier = Modifier.fillMaxWidth())
            SegmentedControl(
                options = listOf(stringResource(R.string.link_show_code), stringResource(R.string.link_enter_code)),
                selected = state.mode.ordinal,
                onSelect = { onMode(LinkMode.entries[it]) },
            )
            when (state.mode) {
                LinkMode.SHOW -> {
                    val now = rememberNow()
                    val remaining = state.windowExpiresAtMs - now
                    if (code != null && remaining > 0) {
                        Text(code.display, style = Warp.codeStyle, color = c.text)
                        QrCode(code.qrPayload, stringResource(R.string.show_qr), Modifier.width(200.dp))
                        Text(
                            stringResource(R.string.link_show_code_body),
                            style = MaterialTheme.typography.bodyMedium,
                            color = c.textSecondary,
                            textAlign = TextAlign.Center,
                        )
                        Text(
                            stringResource(R.string.code_expires_in, Format.countdown(remaining)),
                            style = MaterialTheme.typography.labelSmall,
                            color = c.textTertiary,
                        )
                    } else {
                        Text(stringResource(R.string.link_window_closed), style = MaterialTheme.typography.bodyMedium, color = c.textSecondary)
                    }
                }
                LinkMode.ENTER -> {
                    CodeField(
                        digits = state.input,
                        onChange = onInput,
                        onSubmit = onSubmit,
                        label = stringResource(R.string.link_enter_code),
                    )
                    if (state.busy) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.accent)
                            Text(stringResource(R.string.link_waiting), style = MaterialTheme.typography.bodyMedium, color = c.textSecondary)
                        }
                    }
                    state.error?.let {
                        Text(it.resolve(context), style = MaterialTheme.typography.bodyMedium, color = c.danger, modifier = Modifier.fillMaxWidth())
                    }
                }
            }
        }
    }
}
