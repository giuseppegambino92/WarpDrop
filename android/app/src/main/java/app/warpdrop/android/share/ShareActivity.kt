package app.warpdrop.android.share

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.core.content.IntentCompat
import androidx.core.content.pm.ShortcutManagerCompat
import androidx.core.net.toUri
import androidx.lifecycle.lifecycleScope
import app.warpdrop.android.R
import app.warpdrop.android.core.SendTarget
import app.warpdrop.android.core.Staged
import app.warpdrop.android.core.WarpRepository
import app.warpdrop.android.service.WarpService
import app.warpdrop.android.ui.MainActivity
import app.warpdrop.android.util.EngineErrors
import app.warpdrop.android.util.Locales
import kotlinx.coroutines.launch
import uniffi.warp.ClipboardContent

/**
 * Share Sheet entry point (ACTION_SEND / ACTION_SEND_MULTIPLE).
 *
 * Content URIs are opened right away (the read grant belongs to this
 * activity's intent) and kept open as `/proc/self/fd/N` until the transfer
 * ends. Plain text / links become a clipboard push. With a Direct Share
 * shortcut the send starts immediately; otherwise the items are staged and
 * the main UI asks for a device or a code.
 */
class ShareActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState != null) {
            finish()
            return
        }
        WarpService.start(this) // allowed: this activity is in the foreground
        val shortcutDevice = DirectShare.deviceIdFromShortcut(intent.getStringExtra(ShortcutManagerCompat.EXTRA_SHORTCUT_ID))
        val uris = streamUris(intent)
        val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()?.takeIf { it.isNotBlank() }

        lifecycleScope.launch {
            val staged: Staged = try {
                when {
                    uris.isNotEmpty() -> Staged.Files(SendPreparer(this@ShareActivity).fromUris(uris))
                    text != null -> Staged.Clip(clipContent(text))
                    else -> {
                        finish()
                        return@launch
                    }
                }
            } catch (e: UnreadableItemException) {
                toast(e.text.resolve(this@ShareActivity))
                finish()
                return@launch
            } catch (e: Exception) {
                toast(EngineErrors.fromException(e, null).resolve(this@ShareActivity))
                finish()
                return@launch
            }

            val trusted = shortcutDevice?.let { id ->
                WarpRepository.awaitNode()
                WarpRepository.trusted.value.firstOrNull { it.deviceId == id }
            }
            if (shortcutDevice != null && trusted != null) {
                WarpRepository.stage(staged)
                WarpRepository.sendStaged(SendTarget.Peer(trusted.deviceId, trusted.name))
                toast(Locales.localized(this@ShareActivity).getString(R.string.sending_to, trusted.name))
            } else {
                WarpRepository.stage(staged)
                startActivity(
                    Intent(this@ShareActivity, MainActivity::class.java)
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP),
                )
            }
            finish()
        }
    }

    private fun toast(message: String) {
        Toast.makeText(applicationContext, message, Toast.LENGTH_SHORT).show()
    }

    private fun clipContent(text: String): ClipboardContent {
        val t = text.trim()
        val scheme = t.toUri().scheme?.lowercase()
        return if (!t.any { it.isWhitespace() } && (scheme == "http" || scheme == "https")) {
            ClipboardContent.Url(url = t)
        } else {
            ClipboardContent.Text(text = text)
        }
    }

    private fun streamUris(intent: Intent): List<Uri> {
        val out = LinkedHashSet<Uri>()
        when (intent.action) {
            Intent.ACTION_SEND ->
                IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)?.let(out::add)
            Intent.ACTION_SEND_MULTIPLE ->
                IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)?.let(out::addAll)
        }
        if (out.isEmpty()) {
            intent.clipData?.let { clip ->
                for (i in 0 until clip.itemCount) clip.getItemAt(i).uri?.let(out::add)
            }
        }
        return out.toList()
    }
}
