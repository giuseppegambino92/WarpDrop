package app.warpdrop.android.share

import android.content.Context
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import android.system.Os
import android.system.OsConstants
import android.util.Log
import androidx.documentfile.provider.DocumentFile
import app.warpdrop.android.R
import app.warpdrop.android.util.UiText
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import uniffi.warp.SendItem
import java.io.Closeable
import java.io.File
import java.io.FileInputStream
import java.io.FileNotFoundException
import java.io.FileOutputStream
import java.io.IOException
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Items ready for `WarpNode.sendFiles*`, plus everything that has to stay
 * alive until the transfer reaches a terminal state (open
 * ParcelFileDescriptors behind `/proc/self/fd/N` paths, private copies).
 */
class PreparedSend(
    val items: List<SendItem>,
    val label: UiText,
    private val resources: List<Closeable>,
) : Closeable {
    private val closed = AtomicBoolean(false)

    override fun close() {
        if (closed.compareAndSet(false, true)) {
            resources.forEach { runCatching { it.close() } }
        }
    }
}

/** Thrown when a picked/shared item cannot be read; carries a user-facing message. */
class UnreadableItemException(val text: UiText, cause: Throwable? = null) : IOException(cause)

/**
 * Turns content URIs into [SendItem]s without copying:
 * each document is opened once as a ParcelFileDescriptor and handed to the
 * Rust core as `/proc/self/fd/<fd>` with its DISPLAY_NAME. Only when a
 * provider returns a pipe/socket (not seekable, unknown size) is the stream
 * copied into private storage first.
 *
 * Folders (tree URIs) are walked with DocumentFile: one fd per file, named
 * `<Tree>/<relative path>` (the core recreates the parent folders).
 */
class SendPreparer(context: Context) {
    private val app = context.applicationContext
    private val resolver = app.contentResolver

    suspend fun fromUris(uris: List<Uri>): PreparedSend = withContext(Dispatchers.IO) {
        val items = mutableListOf<SendItem>()
        val resources = mutableListOf<Closeable>()
        try {
            for (uri in uris.distinct()) {
                currentCoroutineContext().ensureActive()
                val (item, res) = openOne(uri)
                items += item
                resources += res
            }
        } catch (e: Throwable) {
            resources.forEach { runCatching { it.close() } }
            throw e
        }
        val label = if (items.size == 1) {
            UiText.Raw(items[0].name ?: File(items[0].path).name)
        } else {
            UiText.res(R.string.items_count, items.size.toString())
        }
        PreparedSend(items, label, resources)
    }

    /**
     * Walks a picked folder (tree URI) and opens one fd per file. Each item is
     * named `<Tree>/<relative path>` so the core rebuilds the hierarchy; no copy.
     * (Empty sub-folders are not represented.)
     */
    suspend fun fromTree(treeUri: Uri): PreparedSend = withContext(Dispatchers.IO) {
        val root = DocumentFile.fromTreeUri(app, treeUri)?.takeIf { it.isDirectory }
            ?: throw UnreadableItemException(UiText.res(R.string.error_folder_unreadable))
        val rootName = safeName(root.name?.takeIf { it.isNotBlank() } ?: "Folder")
        val items = mutableListOf<SendItem>()
        val resources = mutableListOf<Closeable>()
        try {
            walk(root, rootName, items, resources)
        } catch (e: Throwable) {
            resources.forEach { runCatching { it.close() } }
            if (e is UnreadableItemException || e is kotlinx.coroutines.CancellationException) throw e
            throw UnreadableItemException(UiText.res(R.string.error_folder_unreadable), e)
        }
        if (items.isEmpty()) throw UnreadableItemException(UiText.res(R.string.error_folder_unreadable))
        PreparedSend(items, UiText.Raw(rootName), resources)
    }

    private suspend fun walk(dir: DocumentFile, prefix: String, items: MutableList<SendItem>, resources: MutableList<Closeable>) {
        val taken = HashSet<String>()
        for (child in dir.listFiles().sortedBy { it.name.orEmpty() }) {
            currentCoroutineContext().ensureActive()
            val base = safeName(child.name ?: continue)
            var name = base
            var n = 2
            while (!taken.add(name.lowercase())) name = "$base (${n++})"
            val rel = "$prefix/$name"
            if (child.isDirectory) {
                walk(child, rel, items, resources)
            } else if (child.isFile) {
                val (item, res) = openOne(child.uri, rel)
                items += item
                resources += res
            }
        }
    }

    private fun openOne(uri: Uri, nameOverride: String? = null): Pair<SendItem, Closeable> {
        if (uri.scheme == "file") {
            val path = uri.path ?: throw UnreadableItemException(UiText.res(R.string.error_file_unreadable, uri.toString()))
            return SendItem(path = path, name = null) to Closeable {}
        }
        val name = nameOverride ?: displayName(uri)
        val pfd = try {
            resolver.openFileDescriptor(uri, "r")
        } catch (e: Exception) {
            throw UnreadableItemException(UiText.res(R.string.error_file_unreadable, name), e)
        } ?: throw UnreadableItemException(UiText.res(R.string.error_file_unreadable, name))

        if (isReopenableRegularFile(pfd)) {
            return SendItem(path = "/proc/self/fd/${pfd.fd}", name = name) to pfd
        }
        // A pipe or socket: the core needs a sized, re-openable file.
        val staging = newStagingDir()
        try {
            val out = File(staging, safeName(name.substringAfterLast('/')))
            ParcelFileDescriptor.AutoCloseInputStream(pfd).use { input ->
                FileOutputStream(out).use { input.copyTo(it, COPY_BUFFER) }
            }
            return SendItem(path = out.absolutePath, name = name) to Closeable { staging.deleteRecursively() }
        } catch (e: IOException) {
            staging.deleteRecursively()
            throw UnreadableItemException(UiText.res(R.string.error_file_unreadable, name), e)
        }
    }

    private fun isReopenableRegularFile(pfd: ParcelFileDescriptor): Boolean = try {
        val st = Os.fstat(pfd.fileDescriptor)
        OsConstants.S_ISREG(st.st_mode) && FileInputStream("/proc/self/fd/${pfd.fd}").use { true }
    } catch (e: Exception) {
        Log.i(TAG, "fd ${pfd.fd} is not re-openable, copying", e)
        false
    }

    private fun displayName(uri: Uri): String {
        val fromProvider = runCatching {
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                if (c.moveToFirst() && !c.isNull(0)) c.getString(0) else null
            }
        }.getOrNull()
        return fromProvider?.takeIf { it.isNotBlank() }
            ?: uri.lastPathSegment?.substringAfterLast('/')?.takeIf { it.isNotBlank() }
            ?: "file"
    }

    private fun newStagingDir(): File =
        File(outgoingRoot(app), UUID.randomUUID().toString()).apply {
            if (!mkdirs() && !isDirectory) throw FileNotFoundException(path)
        }

    companion object {
        private const val TAG = "SendPreparer"
        private const val COPY_BUFFER = 1 shl 20

        private fun outgoingRoot(context: Context) = File(context.noBackupFilesDir, "outgoing")

        fun cleanStaleOutgoing(context: Context) {
            outgoingRoot(context).deleteRecursively()
        }

        /** Mirrors the core's sanitising so local copies have legal names. */
        fun safeName(name: String): String {
            val cleaned = name.map { ch -> if (ch == '/' || ch == '\u0000') '_' else ch }.joinToString("")
                .trim().trimEnd('.')
            return when (cleaned) {
                "", ".", ".." -> "_"
                else -> cleaned.take(255)
            }
        }
    }
}
