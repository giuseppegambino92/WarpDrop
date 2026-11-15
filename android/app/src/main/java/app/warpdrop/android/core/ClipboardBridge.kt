package app.warpdrop.android.core

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import androidx.core.net.toUri
import uniffi.warp.ClipboardContent
import java.io.File

/** Reads the system clipboard into the core's [ClipboardContent] and back. */
object ClipboardBridge {
    private const val TAG = "ClipboardBridge"

    /** Mirrors the core's MAX_CLIPBOARD (32 MiB). */
    const val MAX_BYTES = 32 * 1024 * 1024

    class TooLargeException : Exception()

    /**
     * Must be called while a WarpDrop activity has input focus (Android 10+
     * only lets the focused app read the clipboard). Returns null when empty.
     */
    fun read(context: Context): ClipboardContent? {
        val cm = context.getSystemService(ClipboardManager::class.java) ?: return null
        val clip = cm.primaryClip ?: return null
        if (clip.itemCount == 0) return null
        val item = clip.getItemAt(0)
        val uri = item.uri
        if (uri != null) {
            val mime = context.contentResolver.getType(uri) ?: clip.description.takeIf { it.mimeTypeCount > 0 }?.getMimeType(0)
            if (mime != null && mime.startsWith("image/")) {
                val bytes = readCapped(context, uri) ?: throw TooLargeException()
                return ClipboardContent.Image(mime = mime, data = bytes)
            }
        }
        val text = (item.text ?: item.coerceToText(context))?.toString()
        if (text.isNullOrBlank()) return null
        return if (looksLikeUrl(text)) ClipboardContent.Url(url = text.trim()) else ClipboardContent.Text(text = text)
    }

    fun write(context: Context, fromName: String, content: ClipboardContent) {
        val app = context.applicationContext
        val clip = when (content) {
            is ClipboardContent.Text -> ClipData.newPlainText(fromName, content.text)
            is ClipboardContent.Url -> ClipData.newPlainText(fromName, content.url)
            is ClipboardContent.Image -> {
                val ext = MimeTypeMap.getSingleton().getExtensionFromMimeType(content.mime) ?: "png"
                val dir = File(app.cacheDir, "clipboard").apply { mkdirs() }
                dir.listFiles()?.forEach { it.delete() }
                val file = File(dir, "clipboard-${System.currentTimeMillis()}.$ext")
                file.writeBytes(content.data)
                val uri = FileProvider.getUriForFile(app, "${app.packageName}.files", file)
                ClipData.newUri(app.contentResolver, fromName, uri)
            }
        }
        Handler(Looper.getMainLooper()).post {
            try {
                app.getSystemService(ClipboardManager::class.java)?.setPrimaryClip(clip)
            } catch (e: Exception) {
                Log.w(TAG, "setPrimaryClip failed", e)
            }
        }
    }

    private fun readCapped(context: Context, uri: Uri): ByteArray? {
        context.contentResolver.openInputStream(uri)?.use { input ->
            val out = java.io.ByteArrayOutputStream()
            val buf = ByteArray(64 * 1024)
            while (true) {
                val n = input.read(buf)
                if (n < 0) break
                out.write(buf, 0, n)
                if (out.size() > MAX_BYTES) return null
            }
            return out.toByteArray()
        }
        return null
    }

    private fun looksLikeUrl(text: String): Boolean {
        val t = text.trim()
        if (t.any { it.isWhitespace() }) return false
        val scheme = t.toUri().scheme?.lowercase() ?: return false
        return scheme == "http" || scheme == "https" || scheme == "warpdrop"
    }
}
