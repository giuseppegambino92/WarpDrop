package app.warpdrop.android.ui.components

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import app.warpdrop.android.R
import app.warpdrop.android.ui.theme.Warp
import app.warpdrop.android.util.Format
import kotlinx.coroutines.delay
import uniffi.warp.CodeInfo
import uniffi.warp.IncomingOffer

/** Ticks once per second while composed. */
@Composable
fun rememberNow(): Long {
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) {
        while (true) {
            now = System.currentTimeMillis()
            delay(1000 - now % 1000)
        }
    }
    return now
}

/** "<Peer> wants to send you <title> (<size>)" + the four SAS emoji. */
@Composable
fun OfferDialog(offer: IncomingOffer, onAccept: () -> Unit, onDecline: () -> Unit) {
    val c = Warp.colors
    val context = LocalContext.current
    AlertDialog(
        onDismissRequest = {},
        containerColor = c.surface,
        shape = Warp.cardShape,
        title = {
            Text(stringResource(R.string.offer_title, offer.peerName), style = MaterialTheme.typography.titleLarge)
        },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(
                    "${offer.title} (${Format.bytes(context, offer.totalBytes)})",
                    style = MaterialTheme.typography.bodyLarge,
                    color = c.text,
                )
                if (offer.sasEmoji.isNotEmpty()) {
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
                        offer.sasEmoji.forEach { Text(it, fontSize = 40.sp) }
                    }
                    Text(
                        stringResource(R.string.offer_sas_hint, offer.peerName),
                        style = MaterialTheme.typography.labelSmall,
                        color = c.textSecondary,
                        textAlign = TextAlign.Center,
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            }
        },
        dismissButton = {
            TextButton(onClick = onDecline, shape = Warp.controlShape) {
                Text(stringResource(R.string.decline), color = c.textSecondary)
            }
        },
        confirmButton = {
            Button(onClick = onAccept, shape = Warp.controlShape, colors = ButtonDefaults.buttonColors(containerColor = c.accent)) {
                Text(stringResource(R.string.accept))
            }
        },
    )
}

/** QR popover: code + QR + "Expires in 7:42". */
@Composable
fun QrDialog(code: CodeInfo, onDismiss: () -> Unit) {
    val c = Warp.colors
    val now = rememberNow()
    Dialog(onDismissRequest = onDismiss) {
        WarpCard {
            Column(Modifier.padding(20.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Text(stringResource(R.string.your_code), style = MaterialTheme.typography.titleSmall, color = c.textSecondary, modifier = Modifier.weight(1f))
                    ToolButton(Icons.Rounded.Close, stringResource(R.string.close), onDismiss)
                }
                Text(code.display, style = Warp.codeStyle, color = c.text)
                Spacer(Modifier.height(16.dp))
                QrCode(code.qrPayload, stringResource(R.string.show_qr), Modifier.width(240.dp))
                Spacer(Modifier.height(12.dp))
                Text(
                    stringResource(R.string.code_expires_in, Format.countdown(code.expiresAtMs.toLong() - now)),
                    style = MaterialTheme.typography.labelSmall,
                    color = c.textSecondary,
                )
            }
        }
    }
}
