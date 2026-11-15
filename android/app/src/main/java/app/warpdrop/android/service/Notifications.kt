package app.warpdrop.android.service

import android.Manifest
import android.app.DownloadManager
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.webkit.MimeTypeMap
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import androidx.core.content.FileProvider
import app.warpdrop.android.R
import app.warpdrop.android.ui.MainActivity
import app.warpdrop.android.util.EngineErrors
import app.warpdrop.android.util.Format
import app.warpdrop.android.util.Locales
import uniffi.warp.CodeInfo
import uniffi.warp.IncomingOffer
import uniffi.warp.TransferDirection
import uniffi.warp.TransferSnapshot
import uniffi.warp.TransferState
import java.io.File

object Notifications {
    const val CHANNEL_STATUS = "status"
    const val CHANNEL_TRANSFERS = "transfers"
    const val CHANNEL_OFFERS = "offers"
    const val CHANNEL_RECEIVED = "received"
    const val CHANNEL_CLIPBOARD = "clipboard"

    const val ID_SERVICE = 1
    private const val ID_CLIPBOARD = 2

    /** (Re)creates channels; names follow the in-app language. */
    fun createChannels(context: Context) {
        val c = Locales.localized(context)
        val nm = context.getSystemService(NotificationManager::class.java) ?: return
        nm.createNotificationChannels(
            listOf(
                NotificationChannel(CHANNEL_STATUS, c.getString(R.string.channel_status), NotificationManager.IMPORTANCE_MIN).apply {
                    setShowBadge(false)
                },
                NotificationChannel(CHANNEL_TRANSFERS, c.getString(R.string.notif_channel_transfers), NotificationManager.IMPORTANCE_LOW).apply {
                    setShowBadge(false)
                },
                NotificationChannel(CHANNEL_OFFERS, c.getString(R.string.channel_offers), NotificationManager.IMPORTANCE_HIGH),
                NotificationChannel(CHANNEL_RECEIVED, c.getString(R.string.channel_received), NotificationManager.IMPORTANCE_DEFAULT),
                NotificationChannel(CHANNEL_CLIPBOARD, c.getString(R.string.channel_clipboard), NotificationManager.IMPORTANCE_DEFAULT),
            ),
        )
    }

    private fun canPost(context: Context): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    private fun post(context: Context, id: Int, n: android.app.Notification) {
        if (!canPost(context)) return
        try {
            NotificationManagerCompat.from(context).notify(id, n)
        } catch (e: SecurityException) {
            // Permission revoked between the check and the call.
        }
    }

    private fun openAppIntent(context: Context, tab: String? = null): PendingIntent {
        val intent = Intent(context, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        if (tab != null) intent.putExtra(MainActivity.EXTRA_TAB, tab)
        return PendingIntent.getActivity(
            context, tab?.hashCode() ?: 0, intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    // ------------------------------------------------------ service status

    /** Persistent notification of the foreground service. */
    fun serviceNotification(context: Context, code: CodeInfo?, active: List<TransferSnapshot>): android.app.Notification {
        val c = Locales.localized(context)
        if (active.isEmpty()) {
            return NotificationCompat.Builder(context, CHANNEL_STATUS)
                .setSmallIcon(R.drawable.ic_stat_warpdrop)
                .setContentTitle(c.getString(R.string.notif_ready))
                .setContentText(code?.let { c.getString(R.string.qs_tile_subtitle, it.display) })
                .setContentIntent(openAppIntent(context))
                .setOngoing(true)
                .setSilent(true)
                .setShowWhen(false)
                .setCategory(NotificationCompat.CATEGORY_SERVICE)
                .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
                .build()
        }
        val total = active.sumOf { it.totalBytes.toLong() }.coerceAtLeast(1)
        val done = active.sumOf { it.doneBytes.toLong() }
        val single = active.singleOrNull()
        val title = if (single != null) {
            val peer = single.peerName.ifBlank { c.getString(R.string.peer_unknown) }
            "${single.title} → $peer"
        } else {
            c.getString(R.string.notif_transfer_running)
        }
        val parts = mutableListOf<String>()
        if (single == null) parts += c.resources.getQuantityString(R.plurals.transfers_count, active.size, active.size)
        val transferring = active.filter { it.state == TransferState.TRANSFERRING }
        if (transferring.isNotEmpty()) {
            parts += Format.speed(c, transferring.sumOf { it.speedBps.toLong() }.toULong())
            single?.etaSecs?.let { parts += Format.eta(c, it) }
        } else if (single != null) {
            parts += stateLabel(c, single)
        }
        single?.path?.let { parts += Format.pathBadge(c, it) }
        val indeterminate = active.all { it.totalBytes == 0uL || it.state == TransferState.CONNECTING || it.state == TransferState.WAITING_FOR_ACCEPT }

        val cancelIntent = Intent(context, NotificationActionReceiver::class.java)
            .setAction(NotificationActionReceiver.ACTION_CANCEL)
            .putExtra(NotificationActionReceiver.EXTRA_ID, single?.id)
        val cancel = PendingIntent.getBroadcast(
            context, 1, cancelIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        return NotificationCompat.Builder(context, CHANNEL_TRANSFERS)
            .setSmallIcon(R.drawable.ic_stat_warpdrop)
            .setContentTitle(title)
            .setContentText(parts.joinToString(" · "))
            .setSubText(if (done > 0) "${Format.bytes(c, done.toULong())} / ${Format.bytes(c, total.toULong())}" else null)
            .setProgress(1000, ((done * 1000) / total).toInt().coerceIn(0, 1000), indeterminate)
            .setContentIntent(openAppIntent(context))
            .addAction(0, c.getString(R.string.cancel), cancel)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setShowWhen(false)
            .setCategory(NotificationCompat.CATEGORY_PROGRESS)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .build()
    }

    private fun stateLabel(c: Context, t: TransferSnapshot): String = when (t.state) {
        TransferState.CONNECTING -> c.getString(R.string.state_connecting)
        TransferState.WAITING_FOR_ACCEPT -> c.getString(R.string.state_waiting, t.peerName.ifBlank { c.getString(R.string.peer_unknown) })
        TransferState.TRANSFERRING -> c.getString(
            if (t.direction == TransferDirection.OUTGOING) R.string.state_transferring else R.string.state_receiving,
        )
        TransferState.PAUSED -> c.getString(R.string.state_paused)
        TransferState.RECONNECTING -> c.getString(R.string.state_reconnecting)
        TransferState.VERIFYING -> c.getString(R.string.state_verifying)
        TransferState.COMPLETED -> c.getString(if (t.direction == TransferDirection.OUTGOING) R.string.state_sent else R.string.state_saved)
        TransferState.FAILED -> c.getString(R.string.state_failed)
        TransferState.CANCELLED -> c.getString(R.string.state_cancelled)
    }

    // -------------------------------------------------------------- offers

    private fun offerNotificationId(offerId: String) = 10_000 + (offerId.hashCode() and 0xFFFF)

    fun showOffer(context: Context, offer: IncomingOffer) {
        val c = Locales.localized(context)
        fun action(action: String, req: Int) = PendingIntent.getBroadcast(
            context, offerNotificationId(offer.offerId) * 2 + req,
            Intent(context, NotificationActionReceiver::class.java)
                .setAction(action)
                .putExtra(NotificationActionReceiver.EXTRA_ID, offer.offerId),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val what = "${offer.title} (${Format.bytes(c, offer.totalBytes)})"
        val emoji = offer.sasEmoji.joinToString("  ")
        val big = buildString {
            append(what)
            if (emoji.isNotEmpty()) {
                append("\n\n").append(emoji).append("\n")
                append(c.getString(R.string.offer_sas_hint, offer.peerName))
            }
        }
        val n = NotificationCompat.Builder(context, CHANNEL_OFFERS)
            .setSmallIcon(R.drawable.ic_stat_warpdrop)
            .setContentTitle(c.getString(R.string.offer_title, offer.peerName))
            .setContentText(if (emoji.isNotEmpty()) "$what  $emoji" else what)
            .setStyle(NotificationCompat.BigTextStyle().bigText(big))
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .setContentIntent(openAppIntent(context))
            .addAction(0, c.getString(R.string.decline), action(NotificationActionReceiver.ACTION_DECLINE, 0))
            .addAction(0, c.getString(R.string.accept), action(NotificationActionReceiver.ACTION_ACCEPT, 1))
            .setAutoCancel(true)
            .setTimeoutAfter(120_000)
            .build()
        post(context, offerNotificationId(offer.offerId), n)
    }

    fun cancelOffer(context: Context, offerId: String) {
        NotificationManagerCompat.from(context).cancel(offerNotificationId(offerId))
    }

    // ------------------------------------------------------------ results

    private fun resultNotificationId(transferId: String) = 20_000 + (transferId.hashCode() and 0xFFFF)

    fun onTransferFinished(context: Context, t: TransferSnapshot, appVisible: Boolean) {
        val c = Locales.localized(context)
        val peer = t.peerName.ifBlank { c.getString(R.string.peer_unknown) }
        val builder = NotificationCompat.Builder(context, CHANNEL_RECEIVED)
            .setSmallIcon(R.drawable.ic_stat_warpdrop)
            .setAutoCancel(true)
        when {
            t.state == TransferState.COMPLETED && t.direction == TransferDirection.INCOMING -> {
                builder.setContentTitle(c.getString(R.string.notif_received_title, peer))
                    .setContentText("${t.title} · ${Format.bytes(c, t.totalBytes)}")
                    .setContentIntent(openResultIntent(context, t.savedPaths) ?: openAppIntent(context))
                    .setCategory(NotificationCompat.CATEGORY_STATUS)
            }
            appVisible -> return
            t.state == TransferState.COMPLETED -> {
                builder.setContentTitle(c.getString(R.string.notif_sent_title, peer))
                    .setContentText(t.title)
                    .setContentIntent(openAppIntent(context))
            }
            t.state == TransferState.FAILED -> {
                builder.setContentTitle("${t.title} — ${c.getString(R.string.state_failed)}")
                    .setContentText(EngineErrors.fromCode(t.errorCode, t.error, t.peerName).resolve(c))
                    .setContentIntent(openAppIntent(context))
                    .setCategory(NotificationCompat.CATEGORY_ERROR)
            }
            else -> return
        }
        post(context, resultNotificationId(t.id), builder.build())
    }

    fun showClipboard(context: Context, fromName: String) {
        val c = Locales.localized(context)
        val n = NotificationCompat.Builder(context, CHANNEL_CLIPBOARD)
            .setSmallIcon(R.drawable.ic_stat_warpdrop)
            .setContentTitle(c.getString(R.string.clipboard_received, fromName))
            .setContentText(c.getString(R.string.clipboard_received_body))
            .setContentIntent(openAppIntent(context))
            .setAutoCancel(true)
            .setTimeoutAfter(60_000)
            .build()
        post(context, ID_CLIPBOARD, n)
    }

    // --------------------------------------------------------- open files

    /** ACTION_VIEW for a single received file, else the system Downloads view. */
    fun viewIntent(context: Context, savedPaths: List<String>): Intent {
        val single = savedPaths.singleOrNull()?.let(::File)
        if (single != null && single.isFile) {
            val uri = runCatching {
                FileProvider.getUriForFile(context, "${context.packageName}.files", single)
            }.getOrNull()
            if (uri != null) {
                val ext = single.extension.lowercase()
                val mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "*/*"
                return Intent(Intent.ACTION_VIEW).setDataAndType(uri, mime)
                    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
            }
        }
        return Intent(DownloadManager.ACTION_VIEW_DOWNLOADS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }

    private fun openResultIntent(context: Context, savedPaths: List<String>): PendingIntent? {
        if (savedPaths.isEmpty()) return null
        val intent = Intent.createChooser(viewIntent(context, savedPaths), null)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        return PendingIntent.getActivity(
            context, savedPaths.hashCode(), intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }
}
