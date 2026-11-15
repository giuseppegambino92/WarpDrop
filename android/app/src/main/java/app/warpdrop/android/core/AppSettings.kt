package app.warpdrop.android.core

import android.content.Context
import android.os.Build
import android.provider.Settings
import androidx.core.content.edit

/** Small persisted preferences. The UI language is stored by AppCompat / LocaleManager. */
class AppSettings(context: Context) {
    private val app = context.applicationContext
    private val prefs = app.getSharedPreferences("settings", Context.MODE_PRIVATE)

    var deviceName: String
        get() = prefs.getString(KEY_NAME, null)?.takeIf { it.isNotBlank() } ?: defaultDeviceName()
        set(value) = prefs.edit { putString(KEY_NAME, value.trim()) }

    var relayUrl: String?
        get() = prefs.getString(KEY_RELAY, null)?.takeIf { it.isNotBlank() }
        set(value) = prefs.edit { putString(KEY_RELAY, value?.trim().orEmpty()) }

    var askedForPermissions: Boolean
        get() = prefs.getBoolean(KEY_ASKED, false)
        set(value) = prefs.edit { putBoolean(KEY_ASKED, value) }

    private fun defaultDeviceName(): String {
        val global = runCatching { Settings.Global.getString(app.contentResolver, Settings.Global.DEVICE_NAME) }.getOrNull()
        if (!global.isNullOrBlank()) return global
        val model = Build.MODEL.orEmpty()
        val maker = Build.MANUFACTURER.orEmpty().replaceFirstChar { it.titlecase() }
        return if (model.startsWith(maker, ignoreCase = true)) model else "$maker $model".trim()
    }

    private companion object {
        const val KEY_NAME = "device_name"
        const val KEY_RELAY = "relay_url"
        const val KEY_ASKED = "asked_permissions"
    }
}
