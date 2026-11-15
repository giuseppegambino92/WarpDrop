package app.warpdrop.android.share

import android.content.Context
import android.content.Intent
import android.util.Log
import androidx.core.content.pm.ShortcutInfoCompat
import androidx.core.content.pm.ShortcutManagerCompat
import androidx.core.graphics.drawable.IconCompat
import app.warpdrop.android.ui.MainActivity
import app.warpdrop.android.util.Format
import uniffi.warp.TrustedDevice

/**
 * Direct Share: one long-lived sharing shortcut per device in My Devices, with
 * the category declared by the `<share-target>` in `res/xml/shortcuts.xml`.
 * Choosing one in the Share Sheet starts ShareActivity with EXTRA_SHORTCUT_ID.
 */
object DirectShare {
    const val CATEGORY = "app.warpdrop.android.category.DEVICE_SHARE_TARGET"
    private const val PREFIX = "device:"
    private const val TAG = "DirectShare"

    @Volatile
    private var lastPublished: List<Pair<String, String>>? = null

    fun deviceIdFromShortcut(shortcutId: String?): String? =
        shortcutId?.takeIf { it.startsWith(PREFIX) }?.removePrefix(PREFIX)

    /** Ranks the device higher in the Share Sheet's Direct Share row. */
    fun reportUsed(context: Context, deviceId: String) {
        if (lastPublished?.any { it.first == deviceId } != true) return
        runCatching { ShortcutManagerCompat.reportShortcutUsed(context, PREFIX + deviceId) }
    }

    fun publish(context: Context, devices: List<TrustedDevice>) {
        val key = devices.map { it.deviceId to it.name }
        if (key == lastPublished) return // setDynamicShortcuts is rate-limited
        lastPublished = key
        try {
            val max = ShortcutManagerCompat.getMaxShortcutCountPerActivity(context).coerceAtLeast(1)
            val shortcuts = devices.take(max).mapIndexed { rank, d ->
                ShortcutInfoCompat.Builder(context, PREFIX + d.deviceId)
                    .setShortLabel(d.name.ifBlank { "WarpDrop" })
                    .setLongLabel(d.name.ifBlank { "WarpDrop" })
                    .setIcon(IconCompat.createWithResource(context, Format.shortcutIcon(d.kind)))
                    .setIntent(
                        Intent(context, MainActivity::class.java)
                            .setAction(Intent.ACTION_VIEW)
                            .putExtra(MainActivity.EXTRA_TAB, MainActivity.TAB_MY_DEVICES),
                    )
                    .setCategories(setOf(CATEGORY))
                    .setLongLived(true)
                    .setRank(rank)
                    .build()
            }
            ShortcutManagerCompat.setDynamicShortcuts(context, shortcuts)
        } catch (e: Exception) {
            lastPublished = null
            Log.w(TAG, "cannot publish sharing shortcuts", e)
        }
    }
}
